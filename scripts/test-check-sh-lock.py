#!/usr/bin/env python3
"""Gate: `scripts/check.sh`'s concurrency lock must name a live holder, and only a live one.

Why this is a gate and not a comment
------------------------------------
The lock is an `flock` on fd 9, and the kernel drops it when the holding process exits -- so the
lock is correct. What was wrong was everything a human or an agent reads to reason about it.

`exec 9>"$_check_lock"` opened the file with `O_TRUNC` **before** `flock` decided anything, so a
run that was about to be refused had already erased the holder's pid. The refusal then reported
`(pid unknown)` -- every time, by construction, not occasionally. Measured 2026-09-11: a push was
refused with `(pid unknown)`, the lock file had been sitting at 0 bytes since 07:24, and the run
that wrote it had exited long before. Nothing on the screen could distinguish "a nine-minute suite
is running, wait for it" from "nobody holds this, go ahead".

The second cost followed from the first: with no way to ask who held it, a waiting agent tested
`[ -e "$lockfile" ]` instead. The file outlives every run, so that wait never ends. Two pushes sat
in a queue behind a lock that had been free for hours.

What it checks
--------------
The real block is lifted out of the real check.sh -- testing a copy would prove nothing about the
file that runs -- and driven as three cases:

1. while a holder is alive, a second run is refused and the message names the holder's actual pid;
2. once the holder exits, the next run acquires immediately, even though the file still exists;
3. a lock file naming a pid that is gone is reported as stale rather than as a live run.
"""

from __future__ import annotations

import os
import pathlib
import re
import select
import subprocess
import sys
import tempfile

REPO = pathlib.Path(__file__).resolve().parent.parent
CHECK_SH = REPO / "scripts" / "check.sh"
START = '_check_lock="${XDG_RUNTIME_DIR:-/tmp}/er-mods-rs-check-sh.lock"'
END = "export ER_CHECK_LOCK_HELD=1"


def lift_block() -> str:
    """The lock preamble, verbatim, with only its path made injectable."""
    source = CHECK_SH.read_text(encoding="utf-8")
    start = source.index(START)
    end = source.index(END, start)
    block = source[start:end].replace(START, '_check_lock="$1"', 1)
    return (
        "#!/usr/bin/env bash\nset -uo pipefail\n"
        + block
        + '\texport ER_CHECK_LOCK_HELD=1\nfi\necho "ACQUIRED by $$"\n'
        + 'sleep "${2:-0}"\n'
    )


def announcement(process: subprocess.Popen, seconds: float = 30.0) -> str:
    """The probe's first line, or "" if it never arrives inside `seconds`.

    A bare `readline()` on a pipe blocks with no bound, and a gate that hangs is worse than a gate
    that fails: it reaches no verdict and stalls every step after it. `select` waits on the
    descriptor itself, so this stays event-driven rather than polling -- which is also what
    `check-no-timeouts.py` requires -- while refusing to wait forever on a probe that died before
    printing anything.
    """
    ready, _, _ = select.select([process.stdout], [], [], seconds)
    if not ready:
        return ""
    return process.stdout.readline()


def probe_env() -> dict:
    """The ambient environment with check.sh's own escape hatches removed.

    `ER_CHECK_LOCK_HELD` is exported by the preamble under test, so a probe launched from a step
    of a running check.sh inherits it and skips the entire lock block -- acquiring nothing and
    reporting success. That is not a weaker version of the test, it is no test at all: measured on
    2026-09-11, case 1 came back `rc 0` where a refusal was required, and the assertions after it
    fell over a tempdir that had already been cleaned. `ER_CHECK_FORCE` goes for the same reason.
    """
    env = dict(os.environ)
    env.pop("ER_CHECK_LOCK_HELD", None)
    env.pop("ER_CHECK_FORCE", None)
    return env


def run(probe: pathlib.Path, lock: pathlib.Path, hold: str = "0") -> subprocess.CompletedProcess:
    return subprocess.run(
        ["bash", str(probe), str(lock), hold],
        capture_output=True,
        text=True,
        timeout=30,
        env=probe_env(),
    )


def main() -> int:
    failures = []

    def check(ok: bool, description: str, detail: str = "") -> None:
        if ok:
            print(f"  ok    {description}")
            return
        print(f"  FAIL  {description}{(' -- ' + detail) if detail else ''}")
        failures.append(description)

    with tempfile.TemporaryDirectory() as tmp:
        probe = pathlib.Path(tmp) / "probe.sh"
        probe.write_text(lift_block(), encoding="utf-8")
        lock = pathlib.Path(tmp) / "check.lock"

        # 1. A live holder is refused, and named.
        # Its own session, so the whole group can be killed: the probe's `sleep` INHERITS fd 9,
        # and an inherited descriptor holds the flock after the shell that opened it is gone.
        # Killing only the shell leaves the lock held by a sleeping child, which is a property of
        # flock rather than a defect -- and the reason check.sh must not hand fd 9 to a long child.
        holder = subprocess.Popen(
            ["bash", str(probe), str(lock), "8"],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            start_new_session=True,
            env=probe_env(),
        )
        # The probe announces itself on stdout once it holds the lock, and a refusal goes to the
        # same merged stream -- so one blocking read is a deterministic readiness signal and can
        # never hang on a probe that failed. Polling the lock file would be neither.
        announced = announcement(holder)
        check(
            announced.startswith("ACQUIRED by "),
            "the winner writes its pid where a contender can read it",
            announced.strip(),
        )

        refused = run(probe, lock)
        check(refused.returncode == 2, "a second run is refused while the lock is held",
              f"rc {refused.returncode}")
        recorded = lock.read_text(encoding="utf-8").strip()
        named = re.search(r"\(pid (\d+)", refused.stderr)
        check(
            named is not None and named.group(1) == recorded,
            "the refusal names the holder's real pid, not `unknown`",
            refused.stderr.splitlines()[0] if refused.stderr else "no message",
        )

        os.killpg(os.getpgid(holder.pid), 15)
        holder.wait(timeout=10)

        # 2. The file outlives the run; the lock does not.
        check(lock.exists(), "the lock file survives its holder, so its presence proves nothing")
        freed = run(probe, lock)
        check(
            freed.returncode == 0 and "ACQUIRED" in freed.stdout,
            "the next run acquires once the holder exits",
            freed.stderr.strip()[:200],
        )

        # 3. A pid that no longer exists is reported as stale, not as a live run. Held by a real
        # second process so the flock is genuinely taken, while the recorded pid is a dead one.
        stale_lock = pathlib.Path(tmp) / "stale.lock"
        holder2 = subprocess.Popen(
            ["bash", str(probe), str(stale_lock), "8"],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            start_new_session=True,
            env=probe_env(),
        )
        check(
            announcement(holder2).startswith("ACQUIRED by "),
            "the second holder takes its own lock before the pid is falsified",
        )
        # A pid that is certainly gone: this process ran, printed its own, and has been reaped.
        dead = subprocess.run(
            ["bash", "-c", "echo $$"], capture_output=True, text=True, timeout=30
        ).stdout.strip()
        stale_lock.write_text(dead + "\n", encoding="utf-8")
        stale = run(probe, stale_lock)
        os.killpg(os.getpgid(holder2.pid), 15)
        holder2.wait(timeout=10)
        check(
            "GONE" in stale.stderr,
            "a recorded pid that has exited is called stale, not a live run",
            stale.stderr.splitlines()[0] if stale.stderr else "no message",
        )

    if failures:
        print(f"check-sh lock: {len(failures)} failure(s)")
        return 1
    print("check-sh lock: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
