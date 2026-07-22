#!/usr/bin/env python3
"""Headless installer for er-net-effects catalogs.

This installer asks for (or accepts CLI arguments for) a catalog directory and an
Elden Ring regulation.bin, rips the SpEffect master/discriminator catalogs into
that directory, and updates er-net-effects.toml so the DLL reads catalogs from the
chosen location.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
DEFAULT_GAME_DIR = Path("/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game")
CONFIG_FILE_NAME = "er-net-effects.toml"
MASTER_CATALOG_FILE_NAME = "er-net-effect-master-catalog.json"
DEFAULT_CONFIG_TEXT = """# er-net-effects standalone DLL configuration.
# Generated/updated by scripts/install-er-net-effects-headless.py.
network_sync = true
overlay_visible_on_start = true
hotkeys_file = '.er-net-effects-hotkeys.json'
selected_effect_file = '.er-net-effects-setting.txt'
selected_catalog_file = '.er-net-effects-catalog-setting.txt'
enabled_file = '.er-net-effects-enabled.txt'
command_file = 'er-net-effects-command.txt'
telemetry_file = 'er-net-effects-telemetry.json'
catalog_dir = 'er-net-effect-catalogs'
master_catalog_file = 'er-net-effect-master-catalog.json'
"""


def first_existing(candidates: list[Path], fallback: Path) -> Path:
    for candidate in candidates:
        if candidate.exists():
            return candidate
    return fallback


def default_paramdef() -> Path:
    return first_existing(
        [
            REPO_ROOT / "resources" / "SpEffect.xml",
            REPO_ROOT
            / ".."
            / "fromsoftware-rs"
            / "tools"
            / "param-generator"
            / "params"
            / "eldenring"
            / "SpEffect.xml",
            Path.home()
            / "projects"
            / "fromsoftware-rs"
            / "tools"
            / "param-generator"
            / "params"
            / "eldenring"
            / "SpEffect.xml",
        ],
        REPO_ROOT / "resources" / "SpEffect.xml",
    )


def default_smithbox_binary_dir() -> Path:
    return first_existing(
        [
            REPO_ROOT / "vendor" / "smithbox",
            REPO_ROOT / "target" / "soulsformats-bridge" / "bin" / "Release" / "net9.0",
            REPO_ROOT.parent / "target" / "soulsformats-bridge" / "bin" / "Release" / "net9.0",
        ],
        REPO_ROOT / "vendor" / "smithbox",
    )


def default_regulation() -> Path:
    return Path(os.environ.get("ER_REGULATION_BIN", DEFAULT_GAME_DIR / "regulation.bin"))


def default_catalog_dir(regulation: Path | None = None) -> Path:
    game_dir = regulation.parent if regulation is not None else DEFAULT_GAME_DIR
    return Path(os.environ.get("ER_NET_EFFECTS_CATALOG_DIR", game_dir / "er-net-effect-catalogs"))


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--catalog-dir", type=Path, help="Directory to write selector *.jsonc catalogs into.")
    parser.add_argument("--regulation-bin", type=Path, help="Path to Elden Ring regulation.bin.")
    parser.add_argument("--config", type=Path, help="er-net-effects.toml to create/update. Defaults beside regulation.bin.")
    parser.add_argument("--paramdef", type=Path, default=default_paramdef(), help="SpEffect.xml paramdef path.")
    parser.add_argument("--smithbox-binary-dir", type=Path, default=default_smithbox_binary_dir(), help="Directory containing Andre.Formats.dll and Andre.SoulsFormats.dll.")
    parser.add_argument("--effects", type=Path, default=REPO_ROOT / "data" / "effects.json", help="Bundled effects.json path.")
    parser.add_argument("--dotnet-bin", default=os.environ.get("DOTNET_BIN", "dotnet"), help="dotnet executable to use.")
    parser.add_argument("--yes", action="store_true", help="Do not prompt for confirmation before writing files.")
    parser.add_argument("--gui", action="store_true", help="Reserved for a future GUI installer; currently unsupported.")
    return parser.parse_args()


def prompt_path(label: str, default: Path, *, must_exist: bool, is_file: bool) -> Path:
    while True:
        prompt = f"{label} [{default}]: "
        value = input(prompt).strip() if sys.stdin.isatty() else ""
        path = Path(value) if value else default
        if must_exist and is_file and not path.is_file():
            print(f"ERROR: file does not exist: {path}", file=sys.stderr)
            if not sys.stdin.isatty():
                raise SystemExit(2)
            continue
        if must_exist and not is_file and not path.exists():
            print(f"ERROR: path does not exist: {path}", file=sys.stderr)
            if not sys.stdin.isatty():
                raise SystemExit(2)
            continue
        return path


def require_file(path: Path, label: str) -> None:
    if not path.is_file():
        raise SystemExit(f"missing {label}: {path}")


def require_smithbox(binary_dir: Path) -> None:
    require_file(binary_dir / "Andre.Formats.dll", "Andre.Formats.dll")
    require_file(binary_dir / "Andre.SoulsFormats.dll", "Andre.SoulsFormats.dll")


def run(command: list[str]) -> None:
    print("running=" + " ".join(str(part) for part in command), flush=True)
    subprocess.run(command, check=True, timeout=30)


def to_runtime_path(path: Path) -> str:
    resolved = path.expanduser().resolve()
    if os.name == "nt":
        return str(resolved)
    if shutil.which("wslpath") is not None:
        result = subprocess.run(
            ["wslpath", "-w", str(resolved)],
            text=True,
            capture_output=True,
            check=False,
            timeout=30,
        )
        if result.returncode == 0:
            return result.stdout.strip()
    return str(resolved)


def toml_single_quoted(value: str) -> str:
    if "'" in value:
        raise SystemExit(
            "cannot write path containing a single quote to the current simple TOML parser: "
            + value
        )
    return f"'{value}'"


def set_toml_key(raw: str, key: str, value: str) -> str:
    replacement = f"{key} = {toml_single_quoted(value)}"
    lines = raw.splitlines()
    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith(f"{key}") and "=" in stripped:
            lines[index] = replacement
            return "\n".join(lines) + "\n"
    if lines and lines[-1].strip():
        lines.append("")
    lines.append(replacement)
    return "\n".join(lines) + "\n"


def update_config(config_path: Path, catalog_dir: Path, master_catalog_path: Path) -> None:
    raw = config_path.read_text(encoding="utf-8") if config_path.exists() else DEFAULT_CONFIG_TEXT
    raw = set_toml_key(raw, "catalog_dir", to_runtime_path(catalog_dir))
    raw = set_toml_key(raw, "master_catalog_file", to_runtime_path(master_catalog_path))
    tmp = config_path.with_suffix(config_path.suffix + ".tmp")
    tmp.write_text(raw, encoding="utf-8")
    tmp.replace(config_path)


def confirm(args: argparse.Namespace, catalog_dir: Path, regulation: Path, config: Path, master: Path) -> None:
    print("catalog_dir=" + str(catalog_dir))
    print("regulation_bin=" + str(regulation))
    print("config=" + str(config))
    print("master_catalog=" + str(master))
    print("runtime_catalog_dir=" + to_runtime_path(catalog_dir))
    print("runtime_master_catalog_file=" + to_runtime_path(master))
    if args.yes or not sys.stdin.isatty():
        return
    answer = input("Write catalogs and update config? [Y/n]: ").strip().lower()
    if answer not in {"", "y", "yes"}:
        raise SystemExit("cancelled")


def main() -> int:
    args = parse_args()
    if args.gui:
        raise SystemExit("GUI installer is not implemented yet; run this headless installer without --gui.")

    regulation = args.regulation_bin or prompt_path(
        "Path to regulation.bin", default_regulation(), must_exist=True, is_file=True
    )
    catalog_dir = args.catalog_dir or prompt_path(
        "Catalog output directory", default_catalog_dir(regulation), must_exist=False, is_file=False
    )
    config = args.config or regulation.parent / CONFIG_FILE_NAME
    master_catalog = catalog_dir / MASTER_CATALOG_FILE_NAME

    require_file(regulation, "regulation.bin")
    require_file(args.paramdef, "SpEffect.xml")
    require_file(args.effects, "effects.json")
    require_smithbox(args.smithbox_binary_dir)
    catalog_dir.mkdir(parents=True, exist_ok=True)
    config.parent.mkdir(parents=True, exist_ok=True)
    confirm(args, catalog_dir, regulation, config, master_catalog)

    run(
        [
            sys.executable,
            str(SCRIPT_DIR / "generate-effect-master-catalog.py"),
            "--regulation",
            str(regulation),
            "--paramdef",
            str(args.paramdef),
            "--effects",
            str(args.effects),
            "--smithbox-binary-dir",
            str(args.smithbox_binary_dir),
            "--dotnet-bin",
            args.dotnet_bin,
            "--output",
            str(master_catalog),
        ]
    )
    run(
        [
            sys.executable,
            str(SCRIPT_DIR / "generate-effect-discriminator-catalogs.py"),
            "--master",
            str(master_catalog),
            "--catalog-dir",
            str(catalog_dir),
            "--effects",
            str(args.effects),
            "--clean",
        ]
    )
    update_config(config, catalog_dir, master_catalog)

    catalog_count = len(list(catalog_dir.glob("*.jsonc")))
    print("installed_catalog_count=" + str(catalog_count))
    print("updated_config=" + str(config))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
