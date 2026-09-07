#!/usr/bin/env python3
"""Sync the built game DLLs to the ~/Elden/main staging dir and regenerate its manifest.

Copies every game-loadable DLL from target/x86_64-pc-windows-msvc/release/ into
ELDEN_MAIN_DIR (default ~/Elden/main) and rewrites dll-deploy-manifest.json with
per-DLL sha256/size plus the repo head. Fails closed if any expected DLL is missing
from the build output (run the release build first).

Usage: python3 scripts/sync-dlls-to-elden-main.py [--dry-run] [--help]
Env:   ELDEN_MAIN_DIR, ER_MODS_REPO_ROOT override the defaults.

An unrecognised argument is an error, not a silent full deploy: this script had no argument
parsing at all, so `--help` copied ten DLLs and rewrote the manifest instead of printing usage.
A deploy is a side effect that must be asked for explicitly.
"""

import hashlib
import importlib.util
import json
import os
import pathlib
import shutil
import subprocess
import sys

SCRIPTS_DIR = pathlib.Path(__file__).resolve().parent

# Crates that ship a loadable DLL but are not me3 natives, so they are absent from the
# me3_shells array `me3-dll-list.py` reads. Exactly the exempt set in
# check-me3-shell-coverage.py that still produces a file the game maps -- currently one:
# the AMD AGS shim, which the game loads by name rather than through an [[natives]] entry.
NON_NATIVE_DLLS = {"amd_ags_x64.dll": "er-ags-stub"}


def _dll_map() -> dict[str, str]:
    """DLL filename -> package, derived from the single source of truth.

    This was a hand-maintained dict of eleven entries while the workspace shipped
    twenty-six shells, so a `sync` deployed eleven DLLs and said nothing about the
    fifteen it left behind at whatever revision they happened to be -- the same silent
    partial-update that `default-members` produces at build time, repeated at deploy
    time. A staging step that can quietly skip a DLL will eventually stage a run whose
    evidence describes code that is not under test, which is indistinguishable from the
    feature not working. Deriving the list means a new shell is covered the moment it
    joins the array in check-rust-build.sh, with no second place to remember.
    """
    spec = importlib.util.spec_from_file_location(
        "me3_dll_list", SCRIPTS_DIR / "me3-dll-list.py"
    )
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    mapping = {f"{artifact}.dll": package for package, artifact in module.dll_pairs()}
    mapping.update(NON_NATIVE_DLLS)
    return mapping


DLLS = _dll_map()


def main() -> int:
    args = sys.argv[1:]
    if "--help" in args or "-h" in args:
        print(__doc__)
        return 0
    unknown = [a for a in args if a != "--dry-run"]
    if unknown:
        # Fail closed. Deploying because an argument was not understood is how a typo becomes a
        # deploy nobody asked for.
        print(f"ERROR: unrecognised argument(s): {' '.join(unknown)}")
        print(__doc__)
        return 2
    dry_run = "--dry-run" in args
    repo = os.environ.get(
        "ER_MODS_REPO_ROOT",
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    )
    release_dir = os.path.join(repo, "target", "x86_64-pc-windows-msvc", "release")
    dest_dir = os.environ.get(
        "ELDEN_MAIN_DIR", os.path.join(os.path.expanduser("~"), "Elden", "main")
    )

    missing = [d for d in DLLS if not os.path.isfile(os.path.join(release_dir, d))]
    if missing:
        print(f"ERROR: missing from {release_dir}: {', '.join(sorted(missing))}")
        print("Run the release build first (cargo xwin build --release ... for each DLL package).")
        return 1

    head = subprocess.run(
        ["git", "-C", repo, "rev-parse", "HEAD"],
        capture_output=True, text=True, check=True, timeout=30,
    ).stdout.strip()

    os.makedirs(dest_dir, exist_ok=True)
    items = []
    for dll in sorted(DLLS):
        src = os.path.join(release_dir, dll)
        dst = os.path.join(dest_dir, dll)
        data = open(src, "rb").read()
        if not dry_run:
            shutil.copyfile(src, dst)
        items.append(
            {
                "bytes": len(data),
                "dll": dll,
                "dst": dst,
                "package": DLLS[dll],
                "sha256": hashlib.sha256(data).hexdigest(),
                "src": src,
            }
        )
        print(f"{'DRY ' if dry_run else ''}synced {dll} ({len(data)} bytes)")

    manifest_path = os.path.join(dest_dir, "dll-deploy-manifest.json")
    if not dry_run:
        with open(manifest_path, "w") as f:
            json.dump({"head": head, "items": items}, f, indent=2, sort_keys=True)
            f.write("\n")
    print(f"{'DRY ' if dry_run else ''}wrote {manifest_path} (head {head[:12]})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
