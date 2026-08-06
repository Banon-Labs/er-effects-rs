#!/usr/bin/env python3
"""Sync the built game DLLs to the ~/Elden/main staging dir and regenerate its manifest.

Copies every game-loadable DLL from target/x86_64-pc-windows-msvc/release/ into
ELDEN_MAIN_DIR (default ~/Elden/main) and rewrites dll-deploy-manifest.json with
per-DLL sha256/size plus the repo HEAD. Fails closed if any expected DLL is missing
from the build output (run the release build first).

Usage: python3 scripts/sync-dlls-to-elden-main.py [--dry-run] [--help]
Env:   ELDEN_MAIN_DIR, ER_EFFECTS_REPO_ROOT override the defaults.

An unrecognised argument is an ERROR, not a silent full deploy: this script had no argument
parsing at all, so `--help` copied ten DLLs and rewrote the manifest instead of printing usage.
A deploy is a side effect that must be asked for explicitly.
"""

import hashlib
import json
import os
import shutil
import subprocess
import sys

# DLL filename -> workspace package that builds it. Extend when a new game DLL crate lands.
DLLS = {
    "amd_ags_x64.dll": "er-ags-stub",
    "er_armament_icons.dll": "er-armament-icons",
    "er_better_refills.dll": "er-better-refills-dll",
    "er_effects_rs.dll": "er-effects-rs",
    "er_input_harness_dll.dll": "er-input-harness-dll",
    "er_inventory_sort.dll": "er-inventory-sort-dll",
    "er_invasion_warp_dll.dll": "er-invasion-warp-dll",
    "er_net_effects_dll.dll": "er-net-effects-dll",
    "er_reload_trace_dll.dll": "er-reload-trace-dll",
    "er_telemetry_dll.dll": "er-telemetry-dll",
    "mushroom_man.dll": "mushroom-man-runtime",
}


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
        "ER_EFFECTS_REPO_ROOT",
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
