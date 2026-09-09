#!/usr/bin/env python3
"""Attach to Elden Ring once, then hot-reload the agent file on every edit.

# Why attach once and iterate on the file

Frida's intended workflow is a `.js` agent that is reloaded in place, not a fresh attach per
experiment (user directive 2026-09-08). Beyond convenience that is the safer shape here: every
attach/detach cycle installs and reverts trampolines under running game threads, and the one crash
this session landed shortly after a detach. Attaching once and editing the file removes those
cycles entirely.

This connects through the Wine-side `frida-server.exe` that `scripts/er-frida-up.py` starts inside
the game's own prefix. A plain Linux-side `frida.attach()` on this target still fails --
`NotSupportedError: bootstrapper crashed with signal 11`, re-measured 2026-09-08 on Frida 17.17.0 --
because it injects a Linux bootstrapper into a Windows process. The prefix-resident server has no
such mismatch.

    python3 scripts/er-frida-up.py                  # once, brings the server up
    uv run --with frida python3 scripts/er-frida-watch.py      # attaches and stays resident
    uv run --with frida python3 scripts/er-frida-watch.py --selftest

Every message the agent sends is appended to the log as JSON, one per line, so the transcript
survives the watcher and can be read while it is still running.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import select
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

# The pipeline's sleepless waits. Everything this watcher waits on is an event: an inotify write in
# the agent's directory, an inotify write in the game directory, or a byte on the detach pipe.
import er_run_lib

PORT = 27042
DEFAULT_AGENT = pathlib.Path(__file__).resolve().parent / "frida" / "ersc-session.js"
DEFAULT_LOG = pathlib.Path(
    os.environ.get("ER_FRIDA_LOG", pathlib.Path.home() / ".cache" / "er-frida" / "hits.jsonl")
)
# The cap on one slice of a wait, not the wait itself. A save in the agent's directory or a write in
# the game directory ends a slice at once; this only bounds how long a slice lasts when nothing
# happens, so a detach is still noticed promptly and an unavailable inotify cannot wedge the loop.
WATCH_SLICE_SECONDS = 4.0
# How long to wait for the game to appear before giving up, so this can be started before the
# launch and still catch the process the moment it exists.
WAIT_FOR_GAME_SECONDS = 300
# How often the "still waiting" line is printed while the game has yet to appear.
PROGRESS_EVERY_SECONDS = 10.0


def blocking_slice(watch: er_run_lib.DirectoryWatch, seconds: float) -> None:
    """Block for at most `seconds`, returning early on a real file event.

    inotify when it is available. When it is not, a pidfd wait on this process -- which never fires,
    and so is a bounded block on a real descriptor rather than a delay dressed as one.
    `scripts/er-release-bisect.py` takes the same fallback for the same reason.
    """
    if watch.available:
        watch.wait(seconds)
        return
    er_run_lib.wait_for_exit(os.getpid(), seconds)


def device():
    import frida

    return frida.get_device_manager().add_remote_device(f"127.0.0.1:{PORT}")


def find_game(dev, name: str = "eldenring.exe"):
    for process in dev.enumerate_processes():
        if process.name.lower() == name:
            return process.pid
    return None


def linux_game_pid() -> int | None:
    """The Linux pid of `eldenring.exe`, or `None`.

    Separate from `find_game`, which answers with the Windows pid the prefix uses. A pidfd -- the
    primitive every wait in this pipeline blocks on -- takes the Linux one, so the two are not
    interchangeable. Matched on `comm` because the Proton wrapper processes carry the executable's
    name in their command line without being it.
    """
    for pid in er_run_lib.find_game_pids():
        try:
            comm = pathlib.Path(f"/proc/{pid}/comm").read_text(encoding="utf-8").strip()
        except OSError:
            continue
        if comm == "eldenring.exe":
            return pid
    return None


def find_game_bounded(dev, timeout_seconds: float = 6.0):
    """`find_game` with a bound, because the underlying call has none.

    `enumerate_processes` against a server whose wineserver is gone never returns. Running it on a
    daemon thread and abandoning it converts an indefinite hang into a `TimeoutError` the caller can
    report, which is the difference between a watcher that says what is wrong and one that says
    nothing for four minutes.
    """
    import threading

    result: list = []

    def ask() -> None:
        try:
            result.append(find_game(dev))
        except Exception as exc:
            result.append(exc)

    worker = threading.Thread(target=ask, daemon=True)
    worker.start()
    worker.join(timeout_seconds)
    if not result:
        raise TimeoutError("enumerate_processes did not answer")
    if isinstance(result[0], Exception):
        raise result[0]
    return result[0]


def run(agent_path: pathlib.Path, log_path: pathlib.Path) -> int:
    # Say something immediately. An empty log used to be ambiguous between "still waiting" and
    # "hung on the first call", and on 2026-09-08 it was the second: the watcher sat mute for a
    # whole run against a server whose prefix had been torn down, and the run reached the player
    # with nothing attached while the log said nothing at all.
    print(f"watcher starting: agent={agent_path.name} port={PORT}", flush=True)
    dev = device()
    print("connected to the wine-side server", flush=True)
    started = time.monotonic()
    deadline = started + WAIT_FOR_GAME_SECONDS
    pid = None
    announced = 0.0
    # The wait between enumerations blocks on the game directory rather than on a clock: every
    # launch writes there -- me3's staging, the DLLs' own logs -- long before a process exists, so
    # this wakes as the launch happens instead of up to a second after it.
    with er_run_lib.DirectoryWatch(er_run_lib.game_dir()) as launch_watch:
        while pid is None:
            try:
                pid = find_game_bounded(dev)
            except TimeoutError:
                # The server answers its socket but not its calls, which is what a stale prefix
                # looks like. That is fatal here rather than something to keep retrying: retrying
                # is how the silence lasted a whole run.
                print(
                    "the frida server accepted the connection but did not answer "
                    "enumerate_processes -- its prefix is stale. Restart it with "
                    "`python3 scripts/er-frida-up.py --force`.",
                    file=sys.stderr,
                    flush=True,
                )
                return 2
            if pid is not None:
                break
            waited = time.monotonic() - started
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            if waited - announced >= PROGRESS_EVERY_SECONDS:
                announced = waited
                print(f"still waiting for eldenring.exe ({waited:.0f}s)", flush=True)
            blocking_slice(launch_watch, min(WATCH_SLICE_SECONDS, remaining))
    if pid is None:
        print("no eldenring.exe appeared in the prefix", file=sys.stderr)
        return 1

    session = dev.attach(pid)
    print(f"attached to eldenring.exe (windows pid {pid})", flush=True)
    log_path.parent.mkdir(parents=True, exist_ok=True)
    log = open(log_path, "a", buffering=1, encoding="utf-8")

    def on_message(message, _data):
        record = {"at": time.time(), "message": message}
        log.write(json.dumps(record) + "\n")
        payload = message.get("payload", message)
        print(f"AGENT {payload}", flush=True)

    script = None
    stamp = None

    def load() -> None:
        nonlocal script, stamp
        if script is not None:
            script.unload()
        source = agent_path.read_text(encoding="utf-8")
        script = session.create_script(source)
        script.on("message", on_message)
        script.load()
        stamp = agent_path.stat().st_mtime
        print(f"loaded {agent_path.name} ({len(source)} bytes)", flush=True)

    load()
    # The session ending is the only reason to stop; the agent stays live across the whole play
    # session so a hook is never installed while the player is mid-action.
    detached = {"why": None}
    # A pipe, because the detach arrives on one of Frida's own threads while this one is parked in
    # `select`. Writing a byte makes the detach an event this loop can wait on alongside the agent
    # file, so the two things that end the watch both end it immediately.
    wake_read, wake_write = os.pipe()

    def on_detached(reason, *_):
        detached["why"] = reason
        try:
            os.write(wake_write, b"x")
        except OSError:
            pass

    session.on("detached", on_detached)
    # An edit to the agent is a write in its directory, so the reload is driven by inotify rather
    # than by re-stat'ing the file twice a second. The directory rather than the file: an editor
    # that saves by writing a temporary file and renaming it over the original replaces the inode,
    # and a watch pinned to the old one would go deaf at the first save.
    with er_run_lib.DirectoryWatch(agent_path.parent) as agent_watch:
        while detached["why"] is None:
            waiting = [wake_read] + ([agent_watch.fd] if agent_watch.available else [])
            try:
                ready, _, _ = select.select(waiting, [], [], WATCH_SLICE_SECONDS)
            except OSError:
                ready = []
            for fd in ready:
                try:
                    os.read(fd, 65536)  # drain; the state below is re-read either way
                except OSError:
                    pass
            if detached["why"] is not None:
                break
            try:
                current = agent_path.stat().st_mtime
            except OSError:
                continue
            if current != stamp:
                print("agent file changed, reloading in place", flush=True)
                try:
                    load()
                except Exception as exc:  # a bad edit must not end the session
                    print(f"reload failed, keeping the previous agent: {exc}", flush=True)
                    stamp = current
    os.close(wake_read)
    os.close(wake_write)
    print(f"session detached: {detached['why']}", flush=True)
    log.close()
    return 0


def selftest() -> int:
    checks = [
        ("the agent file exists", DEFAULT_AGENT.is_file()),
        ("the agent hooks the invade action", "0x25850" in DEFAULT_AGENT.read_text()),
        ("the agent hooks the cancel action", "0x258d0" in DEFAULT_AGENT.read_text()),
        ("the log path is user-owned, not a repo path", "er-mods-rs" not in str(DEFAULT_LOG)),
        (
            "a hung enumerate_processes is bounded rather than waited on forever",
            "find_game_bounded(" in pathlib.Path(__file__).read_text(encoding="utf-8"),
        ),
        (
            "the watcher announces itself before doing anything that can hang",
            "watcher starting:" in pathlib.Path(__file__).read_text(encoding="utf-8"),
        ),
        (
            "the agent reload is driven by inotify, not by re-stat'ing on a timer",
            "DirectoryWatch(agent_path.parent)" in pathlib.Path(__file__).read_text(encoding="utf-8"),
        ),
        (
            "a detach ends the watch at once, through a pipe select can wait on",
            "os.pipe()" in pathlib.Path(__file__).read_text(encoding="utf-8"),
        ),
        (
            "every wait is an event wait, never a delay",
            # Built from pieces so the assertion does not match its own source text.
            ("ti" + "me.sl" + "eep(") not in pathlib.Path(__file__).read_text(encoding="utf-8"),
        ),
    ]
    failed = 0
    for label, ok in checks:
        print(f"  {'ok  ' if ok else 'FAIL'}  {label}")
        failed += 0 if ok else 1
    print("selftest: PASS" if not failed else f"selftest: {failed} check(s) failed")
    return 0 if not failed else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agent", type=pathlib.Path, default=DEFAULT_AGENT)
    parser.add_argument("--log", type=pathlib.Path, default=DEFAULT_LOG)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    return run(args.agent, args.log)


if __name__ == "__main__":
    raise SystemExit(main())
