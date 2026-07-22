#!/usr/bin/env python3
"""Build a user-facing er-effects-rs release helper package.

This package intentionally excludes the actual DLL and all save files. It contains
only docs/examples/launcher glue so it is safe to share without bundling binaries
or user/game save data.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import stat
import subprocess
import sys
import textwrap
import zipfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUT_DIR = REPO_ROOT / "target" / "deliverables"
DEFAULT_NAME = "er-effects-user-release"
FORBIDDEN_EXACT_NAMES = {
    "er_effects_rs.dll",
    "ersc.dll",
    "ER0000.sl2",
    "ER0000.co2",
}
FORBIDDEN_SUFFIXES = {".sl2", ".co2", ".bak", ".dll"}


def git_commit() -> str:
    try:
        return subprocess.check_output(
            ["git", "rev-parse", "--short", "HEAD"],
            cwd=REPO_ROOT,
            text=True,
            stderr=subprocess.DEVNULL,
            timeout=30,
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def write_executable(path: Path, content: str) -> None:
    path.write_text(content, encoding="utf-8")
    mode = path.stat().st_mode
    path.chmod(mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def forbidden_reason(path: Path) -> str | None:
    name = path.name
    if name in FORBIDDEN_EXACT_NAMES:
        return f"forbidden exact name {name}"
    if path.suffix.lower() in FORBIDDEN_SUFFIXES:
        return f"forbidden suffix {path.suffix}"
    return None


def audit_stage(stage_dir: Path) -> None:
    failures: list[str] = []
    for path in sorted(stage_dir.rglob("*")):
        if not path.is_file():
            continue
        reason = forbidden_reason(path)
        if reason is not None:
            failures.append(f"{path.relative_to(stage_dir)}: {reason}")
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        if "savefile = \"\"" in text or "savefile = ''" in text:
            failures.append(f"{path.relative_to(stage_dir)}: empty ME3 savefile override")
    if failures:
        raise SystemExit("release package audit failed:\n" + "\n".join(failures))


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def write_package_files(stage_dir: Path, package_name: str, commit: str) -> None:
    stage_dir.mkdir(parents=True, exist_ok=True)

    (stage_dir / "README.md").write_text(
        textwrap.dedent(
            f"""
            # er-effects-rs user release helper package

            Package: `{package_name}`  
            Source commit: `{commit}`

            This package intentionally does **not** contain:

            - `er_effects_rs.dll`
            - Elden Ring save files (`.sl2`, `.co2`, `.bak`)
            - Seamless Co-op's `ersc.dll` or any other DLL

            ## What this package contains

            - `run-er-effects-release.sh` — Linux/Proton ME3 launcher helper.
            - `quicksave.me3.template` — example ME3 profile with no empty savefile override.
            - `er-effects.toml.example` — optional game-directory config template.
            - `SHA256SUMS.txt` and `PACKAGE-MANIFEST.txt` — package audit artifacts.

            ## Build the DLL yourself

            From the `er-effects-rs` repo:

            ```bash
            cargo xwin build --release --target x86_64-pc-windows-msvc
            ```

            That creates the DLL at:

            ```text
            /home/banon/projects/er-effects-rs/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll
            ```

            ## Launch with ME3

            Run this package's helper and point it at your locally-built DLL:

            ```bash
            ./run-er-effects-release.sh \
              --dll /home/banon/projects/er-effects-rs/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll \
              --steam-dir "$HOME/.local/share/Steam"
            ```

            Optional arguments:

            ```bash
            --slot 0                         # writes slot to the game-directory er-effects.toml
            --save-file /path/to/ER0000.sl2  # optional explicit save path; save is never copied into the package
            --boot-background-image /path/to/background.png
            --me3 /path/to/me3
            --game eldenring
            ```

            The helper generates a runtime-only ME3 profile under `.generated/` next to itself and
            writes `er-effects.toml` in the game directory. Those generated files are local machine
            state and are not part of the release package.
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    (stage_dir / "quicksave.me3.template").write_text(
        textwrap.dedent(
            """
            profileVersion = "v1"
            start_online = false

            [[supports]]
            game = "eldenring"

            [[natives]]
            # Replace this with the absolute path to your locally-built er_effects_rs.dll,
            # or use run-er-effects-release.sh to generate a profile automatically.
            path = '/absolute/path/to/er_effects_rs.dll'
            """
        ).lstrip(),
        encoding="utf-8",
    )

    (stage_dir / "er-effects.toml.example").write_text(
        textwrap.dedent(
            """
            # Optional: copy to er-effects.toml next to eldenring.exe in the game directory,
            # or let run-er-effects-release.sh generate it there.
            # All keys are optional.

            # slot = 0
            # save_file = '/absolute/path/to/ER0000.sl2'
            # boot_background_image = '/absolute/path/to/background.png'
            persist_boot_background_to_loading_screen = true
            """
        ).lstrip(),
        encoding="utf-8",
    )

    write_executable(
        stage_dir / "run-er-effects-release.sh",
        textwrap.dedent(
            r'''#!/usr/bin/env bash
            set -euo pipefail

            ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
            ME3_PATH="${ME3_PATH:-me3}"
            GAME="eldenring"
            DLL_PATH=""
            STEAM_DIR=""
            SLOT=""
            SAVE_FILE=""
            BOOT_BACKGROUND_IMAGE=""
            GAME_DIR=""

            usage() {
              cat <<'USAGE'
            Usage: ./run-er-effects-release.sh --dll /path/to/er_effects_rs.dll [options]

            Required:
              --dll PATH                      Locally-built er_effects_rs.dll; not bundled here

            Options:
              --steam-dir DIR                 Steam root for ME3, e.g. "$HOME/.local/share/Steam"
              --game-dir DIR                  Directory containing eldenring.exe; inferred from --steam-dir when possible
              --save-file PATH                Optional explicit save path; never copied into package
              --slot N                        Optional autoload slot written to generated TOML
              --boot-background-image PATH    Optional boot/loading background image path
              --me3 PATH                      ME3 binary path (default: me3 or $ME3_PATH)
              --game NAME                     ME3 game name (default: eldenring)
              -h, --help                      Show this help

            This helper writes .generated/er-effects.generated.me3 and a game-directory er-effects.toml,
            then runs ME3. It does not set ER_EFFECTS_* product behavior env vars.
            USAGE
            }

            require_value() {
              local flag="$1"
              local value="${2:-}"
              [[ -n "$value" ]] || { echo "$flag requires a value" >&2; exit 2; }
            }

            while [[ $# -gt 0 ]]; do
              case "$1" in
                --dll) require_value "$1" "${2:-}"; DLL_PATH="$2"; shift 2 ;;
                --steam-dir) require_value "$1" "${2:-}"; STEAM_DIR="$2"; shift 2 ;;
                --game-dir) require_value "$1" "${2:-}"; GAME_DIR="$2"; shift 2 ;;
                --save-file) require_value "$1" "${2:-}"; SAVE_FILE="$2"; shift 2 ;;
                --slot) require_value "$1" "${2:-}"; SLOT="$2"; shift 2 ;;
                --boot-background-image) require_value "$1" "${2:-}"; BOOT_BACKGROUND_IMAGE="$2"; shift 2 ;;
                --me3) require_value "$1" "${2:-}"; ME3_PATH="$2"; shift 2 ;;
                --game) require_value "$1" "${2:-}"; GAME="$2"; shift 2 ;;
                -h|--help) usage; exit 0 ;;
                *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
              esac
            done

            [[ -n "$DLL_PATH" ]] || { echo "missing required --dll /path/to/er_effects_rs.dll" >&2; usage >&2; exit 2; }
            DLL_PATH="$(realpath "$DLL_PATH")"
            [[ -f "$DLL_PATH" ]] || { echo "DLL not found: $DLL_PATH" >&2; exit 2; }
            [[ "$(basename "$DLL_PATH")" == "er_effects_rs.dll" ]] || { echo "expected DLL named er_effects_rs.dll: $DLL_PATH" >&2; exit 2; }
            if [[ -n "$SAVE_FILE" ]]; then
              SAVE_FILE="$(realpath "$SAVE_FILE")"
              [[ -f "$SAVE_FILE" ]] || { echo "save file not found: $SAVE_FILE" >&2; exit 2; }
            fi
            if [[ -n "$BOOT_BACKGROUND_IMAGE" ]]; then
              BOOT_BACKGROUND_IMAGE="$(realpath "$BOOT_BACKGROUND_IMAGE")"
              [[ -f "$BOOT_BACKGROUND_IMAGE" ]] || { echo "boot background image not found: $BOOT_BACKGROUND_IMAGE" >&2; exit 2; }
            fi

            if [[ -z "$GAME_DIR" && -n "$STEAM_DIR" && "$GAME" == "eldenring" ]]; then
              GAME_DIR="$STEAM_DIR/steamapps/common/ELDEN RING/Game"
            fi
            [[ -n "$GAME_DIR" ]] || { echo "missing --game-dir DIR (or --steam-dir DIR so the Elden Ring game directory can be inferred)" >&2; exit 2; }
            GAME_DIR="$(realpath "$GAME_DIR")"
            [[ -d "$GAME_DIR" ]] || { echo "game directory not found: $GAME_DIR" >&2; exit 2; }
            [[ -f "$GAME_DIR/eldenring.exe" ]] || { echo "eldenring.exe not found in game directory: $GAME_DIR" >&2; exit 2; }

            GENERATED_DIR="$ROOT/.generated"
            PROFILE_PATH="$GENERATED_DIR/er-effects.generated.me3"
            CONFIG_PATH="$GAME_DIR/er-effects.toml"
            mkdir -p "$GENERATED_DIR"

            python3 - "$PROFILE_PATH" "$CONFIG_PATH" "$DLL_PATH" "$GAME" "$SLOT" "$SAVE_FILE" "$BOOT_BACKGROUND_IMAGE" <<'PY'
            from pathlib import Path
            import json
            import sys

            profile = Path(sys.argv[1])
            config = Path(sys.argv[2])
            dll = sys.argv[3]
            game = sys.argv[4]
            slot = sys.argv[5]
            save_file = sys.argv[6]
            boot_background_image = sys.argv[7]

            profile.write_text(
                'profileVersion = "v1"\n'
                'start_online = false\n\n'
                '[[supports]]\n'
                f'game = {json.dumps(game)}\n\n'
                '[[natives]]\n'
                f'path = {json.dumps(dll)}\n',
                encoding='utf-8',
            )

            lines = [
                '# Generated by run-er-effects-release.sh.',
                '# This file is local machine state; do not redistribute with saves or DLLs.',
            ]
            if slot:
                lines.append(f'slot = {int(slot)}')
            if save_file:
                lines.append(f'save_file = {json.dumps(save_file)}')
            if boot_background_image:
                lines.append(f'boot_background_image = {json.dumps(boot_background_image)}')
            lines.append('persist_boot_background_to_loading_screen = true')
            config.write_text('\n'.join(lines) + '\n', encoding='utf-8')
            PY

            args=()
            if [[ -n "$STEAM_DIR" ]]; then
              args+=(--steam-dir "$STEAM_DIR")
            fi
            args+=(launch -p "$PROFILE_PATH")

            echo "ME3: $ME3_PATH"
            echo "Profile: $PROFILE_PATH"
            echo "Config: $CONFIG_PATH"
            echo "Game directory: $GAME_DIR"
            echo "DLL: $DLL_PATH"
            if [[ -n "$SAVE_FILE" ]]; then echo "Save: $SAVE_FILE"; else echo "Save: default active Steam user save"; fi
            exec "$ME3_PATH" "${args[@]}"
            '''
        ).replace("            ", ""),
    )


def build_package(out_dir: Path, name: str, *, clean: bool) -> tuple[Path, Path]:
    commit = git_commit()
    package_name = f"{name}-{commit}"
    stage_dir = out_dir / package_name
    zip_path = out_dir / f"{package_name}.zip"
    if clean:
        if stage_dir.exists():
            for path in sorted(stage_dir.rglob("*"), reverse=True):
                if path.is_file() or path.is_symlink():
                    path.unlink()
                else:
                    path.rmdir()
            stage_dir.rmdir()
        if zip_path.exists():
            zip_path.unlink()
    if stage_dir.exists() or zip_path.exists():
        raise SystemExit(f"output already exists; pass --clean: {stage_dir} / {zip_path}")

    write_package_files(stage_dir, package_name, commit)
    audit_stage(stage_dir)

    manifest_lines: list[str] = []
    for path in sorted(stage_dir.rglob("*")):
        if path.is_file() and path.name not in {"SHA256SUMS.txt", "PACKAGE-MANIFEST.txt"}:
            manifest_lines.append(f"{path.relative_to(stage_dir).as_posix()}\n")
    (stage_dir / "PACKAGE-MANIFEST.txt").write_text("".join(manifest_lines), encoding="utf-8")

    sums = []
    for path in sorted(stage_dir.rglob("*")):
        if path.is_file() and path.name != "SHA256SUMS.txt":
            sums.append(f"{sha256(path)}  {path.relative_to(stage_dir).as_posix()}\n")
    (stage_dir / "SHA256SUMS.txt").write_text("".join(sums), encoding="utf-8")
    audit_stage(stage_dir)

    with zipfile.ZipFile(zip_path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
        for path in sorted(stage_dir.rglob("*")):
            if path.is_file():
                reason = forbidden_reason(path)
                if reason is not None:
                    raise SystemExit(f"refusing to zip forbidden file {path}: {reason}")
                zf.write(path, path.relative_to(stage_dir).as_posix())

    with zipfile.ZipFile(zip_path) as zf:
        forbidden = [info.filename for info in zf.infolist() if forbidden_reason(Path(info.filename))]
    if forbidden:
        raise SystemExit("zip contains forbidden files:\n" + "\n".join(forbidden))
    return stage_dir, zip_path


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out-dir", type=Path, default=DEFAULT_OUT_DIR)
    parser.add_argument("--name", default=DEFAULT_NAME)
    parser.add_argument("--clean", action="store_true", help="replace existing output for this commit/name")
    args = parser.parse_args(argv)

    stage_dir, zip_path = build_package(args.out_dir.resolve(), args.name, clean=args.clean)
    print(f"stage_dir={stage_dir}")
    print(f"zip_path={zip_path}")
    with zipfile.ZipFile(zip_path) as zf:
        for info in zf.infolist():
            print(f"{info.file_size:9d} {info.filename}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
