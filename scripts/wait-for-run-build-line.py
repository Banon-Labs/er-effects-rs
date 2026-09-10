#!/usr/bin/env python3
"""Wait for a run directory's DLLs to write their `build git=` line, and print the sha.

`scripts/er-run-branch.py` returns once the quickload DLL's own log line confirms it loaded. The
other shells in the profile write their first line a moment later, so anything reading the run's
build sha at that instant is racing them -- and `scripts/er-ship.sh` lost that race twice on
2026-09-10, refusing to push a run that was seconds away from proving itself, with
`still stale after a rebuild (<none>)`.

The wait is on inotify rather than on a poll loop, because a poll loop is a sleep and
`scripts/check-no-timeouts.py` bans those in every language this repo writes. `select` on the
inotify descriptor returns when the kernel says a file in the directory changed, and its timeout
is the safety cap rather than the mechanism.

A `+dirty` line is not an answer: the tree carried uncommitted changes when the DLL was built, so
the binary is not the commit even when the sha matches. That is the same rule
`scripts/check-runtime-evidence.sh` applies, and the two must not disagree about what counts.

    python3 scripts/wait-for-run-build-line.py <run-dir> [--timeout-seconds N]
    python3 scripts/wait-for-run-build-line.py --selftest

Prints the sha and exits 0, or prints nothing and exits 1 if the cap passes first.
"""

from __future__ import annotations

import argparse
import ctypes
import os
import pathlib
import re
import select
import sys
import tempfile

BUILD_LINE = re.compile(r"^build git=([0-9a-f]+)(\+dirty)?\b")
DEFAULT_TIMEOUT_SECONDS = 30
IN_CREATE = 0x100
IN_MODIFY = 0x2
IN_MOVED_TO = 0x80


def build_sha(run_dir: pathlib.Path) -> str | None:
    """The first clean `build git=` sha among the run's DLL logs, or None."""
    for artifact in sorted(run_dir.glob("er-*.log")):
        try:
            with artifact.open(encoding="utf-8", errors="replace") as handle:
                first = handle.readline()
        except OSError:
            continue
        found = BUILD_LINE.match(first)
        if found and not found.group(2):
            return found.group(1)
    return None


def wait_for_build_sha(run_dir: pathlib.Path, timeout_seconds: float) -> str | None:
    """Block until a clean build line exists in `run_dir`, or the cap passes."""
    # Read first: the line is often already there, and arming a watch to learn that is wasteful.
    found = build_sha(run_dir)
    if found is not None:
        return found
    libc = ctypes.CDLL(None, use_errno=True)
    fd = libc.inotify_init1(os.O_CLOEXEC)
    if fd < 0:
        raise OSError(ctypes.get_errno(), "inotify_init1 failed")
    try:
        watch = libc.inotify_add_watch(
            fd, str(run_dir).encode(), IN_CREATE | IN_MODIFY | IN_MOVED_TO
        )
        if watch < 0:
            raise OSError(ctypes.get_errno(), f"inotify_add_watch failed on {run_dir}")
        # Re-read after arming, so a line written between the read above and the watch is not
        # missed -- the classic check-then-watch hole, and the whole point of doing it in this
        # order.
        found = build_sha(run_dir)
        if found is not None:
            return found
        deadline = select.select
        remaining = timeout_seconds
        while remaining > 0:
            start = os.times().elapsed
            ready, _, _ = deadline([fd], [], [], remaining)
            if not ready:
                return None
            # Drain whatever woke us; the event contents do not matter, only that something in the
            # directory changed and the files are worth re-reading.
            os.read(fd, 64 * 1024)
            found = build_sha(run_dir)
            if found is not None:
                return found
            remaining -= max(os.times().elapsed - start, 0.0)
        return None
    finally:
        os.close(fd)


def selftest() -> int:
    failures = 0
    with tempfile.TemporaryDirectory() as raw:
        run = pathlib.Path(raw)
        if build_sha(run) is not None:
            print("  FAIL an empty run directory must have no sha")
            failures = 1
        (run / "er-lockon-filter.log").write_text(
            "build git=7ac6a383a435+dirty module=er_lockon_filter.dll\n", encoding="utf-8"
        )
        if build_sha(run) is not None:
            print("  FAIL a dirty build line must not count")
            failures = 1
        (run / "er-invasion-warp.log").write_text(
            "build git=7ac6a383a435 module=er_invasion_warp.dll base=0x1 pe=0x2 (t)\n",
            encoding="utf-8",
        )
        if build_sha(run) != "7ac6a383a435":
            print("  FAIL a clean line in any er-*.log must count")
            failures = 1
        # The wait returns immediately when the line is already present, with no watch armed.
        if wait_for_build_sha(run, 1.0) != "7ac6a383a435":
            print("  FAIL an already-written line must return at once")
            failures = 1
    with tempfile.TemporaryDirectory() as raw:
        # And it gives up rather than hanging when nothing ever writes one.
        if wait_for_build_sha(pathlib.Path(raw), 0.2) is not None:
            print("  FAIL an empty directory must time out to None")
            failures = 1
    if failures:
        print("wait-for-run-build-line selftest: FAILED")
        return 1
    print("wait-for-run-build-line selftest: OK (5 cases)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("run_dir", nargs="?", help="the br-* artifact directory to watch")
    parser.add_argument("--timeout-seconds", type=float, default=DEFAULT_TIMEOUT_SECONDS)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if not args.run_dir:
        parser.error("a run directory is required unless --selftest is given")
    if args.timeout_seconds <= 0 or args.timeout_seconds > DEFAULT_TIMEOUT_SECONDS:
        parser.error(f"--timeout-seconds must be in (0, {DEFAULT_TIMEOUT_SECONDS}]")
    run_dir = pathlib.Path(args.run_dir)
    if not run_dir.is_dir():
        print(f"no such run directory: {run_dir}", file=sys.stderr)
        return 1
    found = wait_for_build_sha(run_dir, args.timeout_seconds)
    if found is None:
        return 1
    print(found)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
