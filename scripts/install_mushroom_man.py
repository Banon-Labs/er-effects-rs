#!/usr/bin/env python3
# check-no-magic-numbers: allow-file -- installer constants are package-layout and ME3 profile defaults.
"""Install the asset-only Mushroom Man ModEngine2 package.

Expected zip layout:

    install_mushroom_man.py
    mushroom-man/
      mod/
        facegen/facegen.fgbnd.dcx
        parts/fc_m_0000.partsbnd.dcx
        parts/fc_m_0000_l.partsbnd.dcx
        parts/fg_a_0000_m.partsbnd.dcx
        ...optional fallback parts...

The installer copies the bundled `mod` folder to a stable per-user install
folder, then writes a ModEngine2/ME3 profile pointing at that installed package.
It has no third-party Python dependencies.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

PROFILE_NAME = "mushroom-man"
DEFAULT_GAME = "eldenring"
WINDOWS_ME3_RELATIVE_PATH = Path("garyttierney") / "me3" / "bin" / "me3.exe"

REQUIRED_PAYLOAD_FILES = (
    Path("facegen") / "facegen.fgbnd.dcx",
    Path("parts") / "fc_m_0000.partsbnd.dcx",
    Path("parts") / "fc_m_0000_l.partsbnd.dcx",
    Path("parts") / "fg_a_0000_m.partsbnd.dcx",
)
OPTIONAL_PAYLOAD_FILES = (
    Path("parts") / "bd_m_1010.partsbnd.dcx",
    Path("parts") / "bd_m_1010_l.partsbnd.dcx",
)
DISCOVERY_CANDIDATES = (
    Path("mushroom-man") / "mod",
    Path("MushroomMan") / "mod",
    Path("mod"),
    Path("package") / "mod",
)


def emit(message: str) -> None:
    sys.stdout.write(message + "\n")
    sys.stdout.flush()


def fail(message: str) -> None:
    sys.stderr.write("error: " + message + "\n")
    raise SystemExit(1)


def resolve_existing_dir(path: Path, description: str) -> Path:
    resolved = path.expanduser().resolve()
    if not resolved.is_dir():  # pi-lens-ignore: python-thread-global-write — local path validation, no threading
        fail(f"{description} is not a directory: {resolved}")  # pi-lens-ignore: python-thread-global-write — local error construction, no threading
    return resolved


def validate_mod_dir(path: Path) -> list[str]:
    missing = [str(relative) for relative in REQUIRED_PAYLOAD_FILES if not (path / relative).is_file()]
    return missing


def discover_source_mod(bundle_root: Path) -> Path:
    for candidate in DISCOVERY_CANDIDATES:
        path = bundle_root / candidate
        if path.is_dir() and not validate_mod_dir(path):
            return path.resolve()  # pi-lens-ignore: python-thread-global-write — local path result, no threading
    if not validate_mod_dir(bundle_root):
        return bundle_root.resolve()  # pi-lens-ignore: python-thread-global-write — local path result, no threading
    expected = "\n".join(f"  - {candidate}" for candidate in DISCOVERY_CANDIDATES)  # pi-lens-ignore: python-thread-global-write — local message construction, no threading
    fail(
        "could not find bundled mushroom mod payload under any expected path:\n"
        f"{expected}\n"
        "Pass --source-mod <path-to-mod> if your zip uses a different layout."
    )
    raise AssertionError("unreachable")


def default_install_root() -> Path:
    local_app_data = os.environ.get("LOCALAPPDATA")
    if local_app_data:
        return Path(local_app_data) / "MushroomMan"
    return Path.home() / ".local" / "share" / "MushroomMan"


def toml_basic_string(value: str) -> str:
    escaped = value.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'  # pi-lens-ignore: python-thread-global-write — local string return, no threading


def powershell_single_quoted(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def maybe_windows_path(path: Path) -> str:
    resolved = path.expanduser().resolve()
    if os.name == "nt":
        return str(resolved)  # pi-lens-ignore: python-thread-global-write — local path string return, no threading
    wslpath = shutil.which("wslpath")
    if wslpath:  # pi-lens-ignore: python-thread-global-write — local tool discovery branch, no threading
        result = subprocess.run(
            [wslpath, "-w", str(resolved)],
            check=True,
            capture_output=True,
            text=True,
        )
        return result.stdout.strip()
    return str(resolved)


def write_profile(profile_path: Path, package_path_for_me3: str) -> None:
    content = "\n".join(
        (
            'profileVersion = "v1"',
            "natives = []",
            "",
            "[[supports]]",
            f'game = "{DEFAULT_GAME}"',
            "",
            "[[packages]]",
            "enabled = true",
            f"path = {toml_basic_string(package_path_for_me3)}",
            "load_after = []",
            "load_before = []",
            "",
        )
    )
    profile_path.parent.mkdir(parents=True, exist_ok=True)  # pi-lens-ignore: python-thread-global-write — intended installer write path
    profile_path.write_text(content, encoding="utf-8")  # pi-lens-ignore: python-path-traversal — installer writes user-selected profile path


def locate_default_me3() -> Path | None:
    local_app_data = os.environ.get("LOCALAPPDATA")
    if local_app_data:
        candidate = Path(local_app_data) / WINDOWS_ME3_RELATIVE_PATH
        if candidate.is_file():
            return candidate
    wsl_candidate = Path("/mnt/c/Users/choza/AppData/Local") / WINDOWS_ME3_RELATIVE_PATH
    if wsl_candidate.is_file():
        return wsl_candidate
    return None


def write_launcher(script_path: Path, profile_path_for_me3: str, me3_path: Path | None) -> None:
    if me3_path is None:
        me3_expr = "Join-Path $env:LOCALAPPDATA 'garyttierney\\me3\\bin\\me3.exe'"
        me3_line = f"$me3 = {me3_expr}"
    else:
        me3_line = f"$me3 = {powershell_single_quoted(str(me3_path))}"
    content = "\n".join(
        (
            "$ErrorActionPreference = 'Stop'",
            me3_line,
            "if (-not (Test-Path -LiteralPath $me3)) {",
            "  throw \"Could not find me3.exe. Pass --me3 to install_mushroom_man.py or edit this launcher.\"",
            "}",
            f"$profile = {powershell_single_quoted(profile_path_for_me3)}",
            "& $me3 launch -g eldenring --online false -p $profile",
            "",
        )
    )
    script_path.write_text(content, encoding="utf-8")  # pi-lens-ignore: python-path-traversal — installer writes user-selected launcher path


def guarded_remove_tree(path: Path, install_root: Path) -> None:
    expected = install_root / "mod"
    if path.resolve() != expected.resolve():
        fail(f"refusing to remove unexpected path: {path}")
    shutil.rmtree(path)


def copy_payload(source_mod: Path, install_root: Path, force: bool) -> Path:
    destination_mod = install_root / "mod"
    if destination_mod.exists():
        if not force:
            fail(f"install payload already exists: {destination_mod}; rerun with --force to replace it")
        guarded_remove_tree(destination_mod, install_root)
    install_root.mkdir(parents=True, exist_ok=True)
    shutil.copytree(source_mod, destination_mod)
    return destination_mod


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle-root", type=Path, default=Path(__file__).resolve().parent)
    parser.add_argument("--source-mod", type=Path, help="explicit bundled mod folder to install")
    parser.add_argument("--install-root", type=Path, default=default_install_root())
    parser.add_argument("--profile", type=Path, help="profile output path; defaults to <install-root>/mushroom-man.me3")
    parser.add_argument("--profile-name", default=PROFILE_NAME)
    parser.add_argument("--no-copy", action="store_true", help="write a profile pointing directly at --source-mod")
    parser.add_argument("--force", action="store_true", help="replace an existing installed mod folder")
    parser.add_argument("--no-launcher", action="store_true", help="do not write launch_mushroom_man.ps1")
    parser.add_argument("--me3", type=Path, help="explicit path to me3.exe for the generated launcher/--launch")
    parser.add_argument("--launch", action="store_true", help="launch ME3 after writing the profile")
    parser.add_argument("--json", action="store_true", help="write machine-readable JSON summary")
    return parser


def main() -> int:
    args = build_parser().parse_args()
    bundle_root = resolve_existing_dir(args.bundle_root, "bundle root")
    source_mod = resolve_existing_dir(args.source_mod, "source mod") if args.source_mod else discover_source_mod(bundle_root)
    missing = validate_mod_dir(source_mod)
    if missing:
        fail(f"source mod is missing required files: {', '.join(missing)}")

    install_root = args.install_root.expanduser().resolve()
    if args.no_copy:
        installed_mod = source_mod
        install_root.mkdir(parents=True, exist_ok=True)
    else:
        installed_mod = copy_payload(source_mod, install_root, args.force)

    profile_path = args.profile.expanduser().resolve() if args.profile else install_root / f"{args.profile_name}.me3"
    package_path_for_me3 = maybe_windows_path(installed_mod)
    profile_path_for_me3 = maybe_windows_path(profile_path)
    write_profile(profile_path, package_path_for_me3)

    me3_path = args.me3.expanduser().resolve() if args.me3 else locate_default_me3()
    launcher_path = None
    if not args.no_launcher:
        launcher_path = install_root / "launch_mushroom_man.ps1"
        write_launcher(launcher_path, profile_path_for_me3, me3_path)

    warnings = [
        f"optional payload missing: {relative}"
        for relative in OPTIONAL_PAYLOAD_FILES
        if not (installed_mod / relative).is_file()
    ]
    summary = {
        "source_mod": str(source_mod),
        "installed_mod": str(installed_mod),
        "profile": str(profile_path),
        "profile_for_me3": profile_path_for_me3,
        "package_for_me3": package_path_for_me3,
        "launcher": str(launcher_path) if launcher_path else None,
        "me3": str(me3_path) if me3_path else None,
        "warnings": warnings,
    }
    if args.json:
        emit(json.dumps(summary, indent=2))
    else:
        emit("installed Mushroom Man ME3 package")
        emit(f"  source mod:    {summary['source_mod']}")
        emit(f"  installed mod: {summary['installed_mod']}")
        emit(f"  profile:       {summary['profile']}")
        if launcher_path:
            emit(f"  launcher:      {launcher_path}")
        for warning in warnings:
            emit(f"  warning:       {warning}")

    if args.launch:
        if me3_path is None:
            fail("--launch requested, but me3.exe was not found; pass --me3 <path>")
        subprocess.run(
            [str(me3_path), "launch", "-g", DEFAULT_GAME, "--online", "false", "-p", profile_path_for_me3],
            check=True,
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
