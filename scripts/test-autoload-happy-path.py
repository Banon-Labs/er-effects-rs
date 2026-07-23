#!/usr/bin/env python3
"""Smoke-test the clean me3 autoload release staging path."""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
STAGE_SCRIPT = REPO_ROOT / "scripts" / "stage-autoload-release.sh"

EXPECTED_PROFILE = """profileVersion = "v1"

[[supports]]
game = "eldenring"

[[natives]]
path = 'er_effects_rs.dll'
"""

EXPECTED_AUTOLOAD = """# Product/default zero-input gold-load request.
# Do not set the direct-menu-load method here: that arms the experimental product_core/menu path only
# when er-effects-experimental-direct-menu-load.txt or ER_EFFECTS_EXPERIMENTAL_DIRECT_MENU_LOAD=1 is
# also present. The supported path keeps product_core off and uses the native Continue/PAB gates.
slot=0
"""

EXPECTED_NATIVE_CONTINUE = """# Copy to er-effects-native-continue.txt next to eldenring.exe to enable the supported
# zero-input native Continue path.
"""

EXPECTED_PAB_ADVANCE = """# Copy to er-effects-pab-advance.txt next to eldenring.exe to enable the supported
# zero-input press-any-button/menu-open advance.
"""

EXPECTED_SPLASH_SKIP = """# Copy this file to er-effects-splash-skip.txt next to eldenring.exe to enable
# er-effects-rs' built-in current-version splash skip patch.
"""


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="er-effects-autoload-stage-") as tmp:
        tmp_path = Path(tmp)
        er_dll = tmp_path / "er_effects_rs.dll"
        er_dll.write_bytes(b"fake er effects dll\n")
        out = tmp_path / "release"

        env = os.environ.copy()
        env["ER_EFFECTS_DLL"] = str(er_dll)
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
            "er_effects_rs.dll",
            "er-effects.me3",
            "er-effects-autoload.txt.example",
            "er-effects-native-continue.txt.example",
            "er-effects-pab-advance.txt.example",
            "er-effects-splash-skip.txt.example",
            "SHA256SUMS.txt",
        }
        actual_files = {str(path.relative_to(out)) for path in out.rglob("*") if path.is_file()}
        if actual_files != expected_files:
            raise SystemExit(f"unexpected staged files: {sorted(actual_files)}")
        for name in ("dinput8.dll", "lazyLoad.ini"):
            if (out / name).exists():
                raise SystemExit(f"LazyLoader artifact {name} must not be staged (removed 2026-07-04)")
        if (out / "er-effects.me3").read_text(encoding="utf-8") != EXPECTED_PROFILE:
            raise SystemExit("er-effects.me3 is not the clean single-native profile (relative DLL path)")
        if (out / "er-effects-autoload.txt.example").read_text(encoding="utf-8") != EXPECTED_AUTOLOAD:
            raise SystemExit("autoload example must keep direct_menu_load/product_core off by default")
        if (out / "er-effects-native-continue.txt.example").read_text(encoding="utf-8") != EXPECTED_NATIVE_CONTINUE:
            raise SystemExit("native-continue example does not document the supported zero-input path")
        if (out / "er-effects-pab-advance.txt.example").read_text(encoding="utf-8") != EXPECTED_PAB_ADVANCE:
            raise SystemExit("pab-advance example does not document the supported zero-input path")
        if (out / "er-effects-splash-skip.txt.example").read_text(encoding="utf-8") != EXPECTED_SPLASH_SKIP:
            raise SystemExit("splash-skip example does not document the built-in current-version patch")
        if shutil.which("sha256sum") is None:
            raise SystemExit("sha256sum unexpectedly unavailable after staging")

    print("autoload release staging smoke passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
