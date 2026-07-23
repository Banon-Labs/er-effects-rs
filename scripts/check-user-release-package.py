#!/usr/bin/env python3
"""Smoke/audit the user release helper package."""

from __future__ import annotations

import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
BUILDER = REPO_ROOT / "scripts" / "build-user-release-package.py"
OUT_DIR = REPO_ROOT / "target" / "check-user-release-package"
FORBIDDEN_EXACT = {"er_effects_rs.dll", "ER0000.sl2", "ER0000.co2", "ersc.dll"}
FORBIDDEN_SUFFIXES = {".dll", ".sl2", ".co2", ".bak"}
REQUIRED = {
    "README.md",
    "run-er-effects-release.sh",
    "quicksave.me3.template",
    "er-effects.toml.example",
    "SHA256SUMS.txt",
    "PACKAGE-MANIFEST.txt",
}


def forbidden(name: str) -> str | None:
    path = Path(name)
    if path.name in FORBIDDEN_EXACT:
        return f"forbidden exact name {path.name}"
    if path.suffix.lower() in FORBIDDEN_SUFFIXES:
        return f"forbidden suffix {path.suffix}"
    return None


def main() -> int:
    if OUT_DIR.exists():
        shutil.rmtree(OUT_DIR)
    OUT_DIR.mkdir(parents=True)
    result = subprocess.run(
        [sys.executable, str(BUILDER), "--out-dir", str(OUT_DIR), "--name", "smoke", "--clean"],
        cwd=REPO_ROOT,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        timeout=30,
    )
    print(result.stdout, end="")
    stage_lines = [line for line in result.stdout.splitlines() if line.startswith("stage_dir=")]
    zip_lines = [line for line in result.stdout.splitlines() if line.startswith("zip_path=")]
    if not stage_lines:
        raise AssertionError("builder did not print stage_dir=")
    if not zip_lines:
        raise AssertionError("builder did not print zip_path=")
    stage_dir = Path(stage_lines[-1].split("=", 1)[1])
    zip_path = Path(zip_lines[-1].split("=", 1)[1])
    if not zip_path.exists():
        raise AssertionError(f"zip was not created: {zip_path}")
    with zipfile.ZipFile(zip_path) as zf:
        names = {info.filename for info in zf.infolist()}
        missing = REQUIRED - names
        if missing:
            raise AssertionError(f"release package missing required files: {sorted(missing)}")
        failures = [f"{name}: {reason}" for name in sorted(names) if (reason := forbidden(name))]
        if failures:
            raise AssertionError("release package included forbidden files:\n" + "\n".join(failures))
        for name in names:
            text = zf.read(name).decode("utf-8", errors="replace")
            if "savefile = \"\"" in text or "savefile = ''" in text:
                raise AssertionError(f"{name} contains empty ME3 savefile override")
    subprocess.run(["shellcheck", str(stage_dir / "run-er-effects-release.sh")], check=True, timeout=30)
    print(f"user release package audit passed: {zip_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
