#!/usr/bin/env python3
"""Report which `er_effects_rs.dll` a live Elden Ring actually mapped, and whether it
is the build you meant to test.

Why this exists (2026-08-12): `~/Elden/launch.sh`'s default profile hard-codes the MAIN
tree's `target/x86_64-pc-windows-msvc/release/er_effects_rs.dll`. Launching it from a
worktree silently validates main's build instead of the branch's, and a whole runtime
result got attributed to a fix that was never loaded. The DLL debug log's per-line
`dll:<8 hex>` tag IS the first 8 hex of the loaded file's md5 (see
`crates/er-effects-rs/src/telemetry/save_policy_logs.rs::dll_md5_short`), so the mapped
file, its md5, and that tag are all cross-checkable -- this does it in one command.

Usage:
    python3 scripts/which-dll-is-loaded.py                      # report what is mapped
    python3 scripts/which-dll-is-loaded.py --expect <dll path>  # and assert it matches

Exit codes: 0 match / report-only, 1 mismatch, 2 no live game or no DLL mapped.
"""

from __future__ import annotations

import argparse
import glob
import hashlib
import os
import re
import sys

GAME_COMM = "eldenring.exe"
DLL_NAME = "er_effects_rs.dll"


def live_game_pids() -> list[int]:
    """PIDs whose comm is exactly the game. Deliberately not `pgrep` -- a raw pgrep both
    self-matches and trips this workspace's guard policy."""
    pids: list[int] = []
    for entry in glob.glob("/proc/[0-9]*"):
        try:
            with open(os.path.join(entry, "comm"), encoding="utf-8") as handle:
                comm = handle.read().strip()
        except OSError:
            continue
        if comm == GAME_COMM:
            pids.append(int(os.path.basename(entry)))
    return sorted(pids)


def mapped_dll_paths(pid: int) -> list[str]:
    """Distinct on-disk paths for the mapped DLL, in first-seen order."""
    pattern = re.compile(r"(\S*" + re.escape(DLL_NAME) + r")$")
    seen: list[str] = []
    try:
        with open(f"/proc/{pid}/maps", encoding="utf-8", errors="replace") as handle:
            for line in handle:
                match = pattern.search(line.rstrip())
                if match and match.group(1) not in seen:
                    seen.append(match.group(1))
    except OSError as error:
        print(f"cannot read /proc/{pid}/maps: {error}", file=sys.stderr)
    return seen


def md5_of(path: str) -> str | None:
    try:
        with open(path, "rb") as handle:
            return hashlib.md5(handle.read()).hexdigest()
    except OSError:
        return None


def describe(path: str) -> str:
    digest = md5_of(path)
    if digest is None:
        return f"{path}\n    (unreadable -- cannot hash)"
    return f"{path}\n    md5={digest}  log tag `dll:{digest[:8]}`"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--expect",
        metavar="DLL",
        help="path to the DLL you intended to test; exit 1 unless the game mapped it",
    )
    args = parser.parse_args()

    pids = live_game_pids()
    if not pids:
        print(f"no live {GAME_COMM}", file=sys.stderr)
        return 2

    loaded: dict[str, str | None] = {}
    for pid in pids:
        paths = mapped_dll_paths(pid)
        if not paths:
            print(f"pid {pid}: no {DLL_NAME} mapped")
            continue
        for path in paths:
            print(f"pid {pid} mapped {describe(path)}")
            loaded[path] = md5_of(path)

    if not loaded:
        print(f"no {DLL_NAME} mapped by any live game", file=sys.stderr)
        return 2

    if not args.expect:
        return 0

    expected_path = os.path.abspath(args.expect)
    expected_md5 = md5_of(expected_path)
    if expected_md5 is None:
        print(f"MISMATCH: expected DLL is unreadable: {expected_path}", file=sys.stderr)
        return 1
    if expected_md5 in loaded.values():
        print(f"OK: the game mapped the expected build (md5={expected_md5})")
        return 0
    print(
        "MISMATCH: the game did NOT map the expected build.\n"
        f"  expected {expected_path}\n           md5={expected_md5}\n"
        "  mapped   " + "\n           ".join(f"{p} md5={m}" for p, m in loaded.items()) + "\n"
        "  Any runtime result from this process describes the MAPPED build, not the expected one.\n"
        "  Fix: point the me3 profile's [[natives]] path at the build you mean, or launch with\n"
        "  ME3_PROFILE=<profile naming that path> ~/Elden/launch.sh",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
