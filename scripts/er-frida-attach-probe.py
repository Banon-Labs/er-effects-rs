#!/usr/bin/env python3
"""Test whether `frida.attach()` survives against the Wine/Proton `eldenring.exe`.

# Why this exists

A `bd` memory recorded on 2026-08-12 said a plain `frida.attach()` on this target injects a
bootstrapper that segfaults inside the game and kills it instantly, and that memory became the
reason every live-memory question in this repo goes through `/proc/<pid>/mem` instead. The user
deleted that memory on 2026-09-08 and asked for the claim to be re-established rather than
inherited: Frida is 17.17.0 now, the August failure would have been on a 16.x, and a working Frida
would replace a 179,476-candidate differential memory scan with one `Interceptor.attach` that reads
`rdi` at `ersc+0x25850`.

So this script does the one thing nobody wants to do by accident, deliberately and in isolation:
attach to a throwaway session and report whether the process is still alive afterwards. It reads
nothing about the game's state and changes nothing. The only output that matters is the pid before
and the pid after.

    uv run --with frida python3 scripts/er-frida-attach-probe.py
    uv run --with frida python3 scripts/er-frida-attach-probe.py --selftest

`--selftest` runs the same attach against a child of this process, which proves the probe's own
mechanism works and keeps a failure against the game from being read as a broken probe.

This probe answers one narrow question and should not be the reason anyone attaches. The working
path on this target is `scripts/er-frida-up.py`, which runs a Windows `frida-server.exe` inside the
game's pressure-vessel container and never injects a Linux bootstrapper at all.
"""

from __future__ import annotations

import argparse
import pathlib
import subprocess
import sys
import threading
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

# `wait_for_exit` blocks on a pidfd, which is the readiness primitive for the one question this
# probe asks: did the target die.
import er_run_lib

# Wine reports the Windows executable name here; the `exe` symlink points at wine64-preloader for
# every Windows process in the prefix, so `comm` is the only usable discriminator.
PROCESS_NAME = "eldenring.exe"
# How long the target is watched after the attach before a verdict is given.
#
# This was 3 seconds and that was too short to support the word it produced. On 2026-09-08 the
# probe reported "attach is survivable on this stack" because the pid was still there three seconds
# later; the game then died about a minute afterwards. Three seconds cannot distinguish "the attach
# was harmless" from "the attach left the process damaged and it has not noticed yet", and the
# stronger claim is the one a reader takes away.
#
# The repo caps non-game commands at 30 seconds, so the probe cannot watch for a minute in one
# call. It watches as long as it can and then says what that does and does not establish, rather
# than rounding a short observation up to a verdict.
SETTLE_SECONDS = 20
# How long the injected script gets to say it ran. Its `send` is the readiness signal that the
# attach did something, and a bootstrapper that crashed answers with silence -- so this is the bound
# on the silence, not a pause to let anything settle.
AGENT_REPLY_SECONDS = 5


def find_pid(process: str) -> int | None:
    for entry in pathlib.Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            if (entry / "comm").read_text(encoding="utf-8").strip() == process:
                return int(entry.name)
        except OSError:
            continue
    return None


def alive(pid: int) -> bool:
    return pathlib.Path(f"/proc/{pid}").exists()


def probe(pid: int, label: str) -> bool:
    """Attach, run the smallest possible script, detach. Returns whether the target survived."""
    import frida

    print(f"{label}: pid {pid} alive before = {alive(pid)}", flush=True)
    try:
        session = frida.attach(pid)
        print(f"{label}: ATTACHED", flush=True)
        script = session.create_script(
            "send({modules: Process.enumerateModules().length});"
        )
        answered = threading.Event()

        def note(message, _data):
            print(f"{label}: {message}", flush=True)
            answered.set()

        script.on("message", note)
        script.load()
        # Wait for the agent's own message rather than for a second to pass. The message is proof
        # the injected script ran, so a working attach is confirmed the moment it works and a
        # bootstrapper that crashed is reported as silence instead of being detached out from under.
        if not answered.wait(AGENT_REPLY_SECONDS):
            print(
                f"{label}: the script loaded but sent nothing within {AGENT_REPLY_SECONDS}s",
                flush=True,
            )
        session.detach()
        print(f"{label}: detached cleanly", flush=True)
    except Exception as exc:  # the failure is the measurement, so it is reported, not raised
        print(f"{label}: ATTACH FAILED -- {type(exc).__name__}: {exc}", flush=True)
    # The verdict is "did this process die", and a pidfd is the readiness primitive for precisely
    # that: it becomes readable the instant the target exits. So the window is now a bound on the
    # watch rather than its mechanism -- a kill is reported when it happens, with how long it took,
    # instead of being inferred from a `/proc` entry at the end of a fixed wait.
    started = time.monotonic()
    exited = er_run_lib.wait_for_exit(pid, SETTLE_SECONDS)
    if exited:
        print(
            f"{label}: pid {pid} exited {time.monotonic() - started:.1f}s after the attach",
            flush=True,
        )
    survived = not exited
    print(f"{label}: pid {pid} alive after = {survived}", flush=True)
    return survived


def selftest() -> int:
    # A child that blocks on stdin, not on a timer. It cannot outlive the probe -- closing the pipe
    # or killing it ends it -- and it cannot end mid-probe either, which a fixed-duration
    # child could the moment the settle window grew past its own lifetime.
    child = subprocess.Popen(
        [sys.executable, "-c", "import sys; sys.stdin.read()"], stdin=subprocess.PIPE
    )
    try:
        survived = probe(child.pid, "selftest")
    finally:
        child.kill()
        child.wait()
    if not survived:
        print("selftest FAILED: the probe killed an ordinary child process", file=sys.stderr)
        return 1
    print("selftest ok: attach + detach leaves an ordinary process running")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--process", default=PROCESS_NAME)
    parser.add_argument("--pid", type=int)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    pid = args.pid or find_pid(args.process)
    if pid is None:
        print(f"no {args.process} running", file=sys.stderr)
        return 1
    survived = probe(pid, "eldenring")
    if not survived:
        print(f"VERDICT: the game died within {SETTLE_SECONDS}s of the attach")
        return 2
    print(
        f"VERDICT: the game was still alive {SETTLE_SECONDS}s after the attach. That rules out an "
        "immediate kill and NOTHING MORE -- a crashed bootstrapper can leave the process damaged "
        "and take it down later, which is what happened on 2026-09-08. Do not read this as "
        "permission to attach to a session you care about."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
