#!/usr/bin/env python3
"""Smoke-test the clean LazyLoader autoload release staging path."""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
STAGE_SCRIPT = REPO_ROOT / "scripts" / "stage-autoload-release.sh"

EXPECTED_LAZYLOAD = """; LazyLoader by Church Guard
; er-quickload-rs must be properly loaded, not lazy-loaded, so it is the CHAINLOAD DLL.
; Put additional LazyLoader mods in dllMods and list them under [LOADORDER].

[LAZYLOAD]
dllModFolderName=dllMods

[LOADORDER]

[CHAINLOAD]
dll=er_quickload_rs.dll
"""

EXPECTED_AUTOLOAD = """slot=0
method=direct_menu_load
require_title_bootstrap=false
"""

EXPECTED_SPLASH_SKIP = """# Copy this file to er-effects-splash-skip.txt next to eldenring.exe to enable
# er-quickload-rs' built-in current-version splash skip patch.
"""


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="er-effects-autoload-stage-") as tmp:
        tmp_path = Path(tmp)
        lazyloader = tmp_path / "lazyloader"
        lazyloader.mkdir()
        (lazyloader / "dinput8.dll").write_bytes(b"fake lazyloader proxy\n")
        quickload_dll = tmp_path / "er_quickload_rs.dll"
        quickload_dll.write_bytes(b"fake er quickload dll\n")
        out = tmp_path / "release"

        env = os.environ.copy()
        env["LAZYLOADER_DIR"] = str(lazyloader)
        env["ER_QUICKLOAD_DLL"] = str(quickload_dll)
        result = subprocess.run(
            [str(STAGE_SCRIPT), "--no-build", "--output", str(out)],
            cwd=REPO_ROOT,
            env=env,
            text=True,
            capture_output=True,
            timeout=30,
            check=False,
        )
        if result.returncode != 0:
            raise SystemExit(
                f"stage script failed rc={result.returncode}\nSTDOUT:\n{result.stdout}\nSTDERR:\n{result.stderr}"
            )

        expected_files = {
            "dinput8.dll",
            "lazyLoad.ini",
            "er_quickload_rs.dll",
            "er-effects-autoload.txt.example",
            "er-effects-splash-skip.txt.example",
            "SHA256SUMS.txt",
        }
        actual_files = {str(path.relative_to(out)) for path in out.rglob("*") if path.is_file()}
        if actual_files != expected_files:
            raise SystemExit(f"unexpected staged files: {sorted(actual_files)}")
        if (out / "lazyLoad.ini").read_text(encoding="utf-8") != EXPECTED_LAZYLOAD:
            raise SystemExit("lazyLoad.ini is not the clean single-DLL config")
        if (out / "er-effects-autoload.txt.example").read_text(encoding="utf-8") != EXPECTED_AUTOLOAD:
            raise SystemExit("autoload example is not the product direct_menu_load request")
        if (out / "er-effects-splash-skip.txt.example").read_text(encoding="utf-8") != EXPECTED_SPLASH_SKIP:
            raise SystemExit("splash-skip example does not document the built-in current-version patch")
        if list((out / "dllMods").glob("*.dll")):
            raise SystemExit("dllMods should be empty in the clean er-quickload-rs chainload package")
        if shutil.which("sha256sum") is None:
            raise SystemExit("sha256sum unexpectedly unavailable after staging")

    print("autoload release staging smoke passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
