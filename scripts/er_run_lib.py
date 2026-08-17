"""Shared primitives for the branch-launch pipeline: sleepless waits and run state.

TWO RULES SHAPE EVERYTHING HERE
-------------------------------
1. **No sleeps as synchronization** (`scripts/check-no-timeouts.py`). Readiness is an
   *event*; a timeout is only a safety backstop. So waiting for a log line blocks on inotify
   and waiting for a process to exit blocks on a pidfd -- in both cases `select()` returns
   the instant the thing happens, and the timeout exists solely so a wedged run cannot hang
   a caller forever.
2. **Every agent-facing shell op is capped at 30s.** Callers pass bounds under that; the
   waits below are written so a caller can loop over short bounded waits and stay responsive
   rather than asking for one long one.

This module exists because seven scripts in this directory each grew their own copy of the
same ctypes inotify block. New code in this pipeline uses this one instead of making it
eight.
"""

from __future__ import annotations

import ctypes
import ctypes.util
import errno
import json
import os
import select
from dataclasses import dataclass, field
from pathlib import Path

# <sys/inotify.h>
IN_MODIFY = 0x0000_0002
IN_CREATE = 0x0000_0100
IN_MOVED_TO = 0x0000_0080
IN_CLOSE_WRITE = 0x0000_0008
DEFAULT_MASK = IN_MODIFY | IN_CREATE | IN_MOVED_TO | IN_CLOSE_WRITE

RUN_STATE_ROOT = Path(
    os.environ.get("ER_RUN_STATE_DIR", Path.home() / ".cache" / "er-me3-runs")
)


class DirectoryWatch:
    """Block until something in `directory` changes, without polling.

    Watches the DIRECTORY rather than a file: the DLL rotates its logs at startup
    (`<name>.log` -> `<name>.log.prev`), so a watch pinned to an inode would go deaf at
    exactly the moment the interesting run begins.

    Degrades honestly: if inotify is unavailable, `available` is False and `wait()` returns
    immediately, so a caller's own bounded re-check loop still makes progress instead of the
    wait silently blocking forever.
    """

    def __init__(self, directory: Path, mask: int = DEFAULT_MASK) -> None:
        self.directory = Path(directory)
        self.fd = -1
        self._libc = None
        try:
            self._libc = ctypes.CDLL(ctypes.util.find_library("c"), use_errno=True)
            self._libc.inotify_init1.argtypes = [ctypes.c_int]
            self._libc.inotify_init1.restype = ctypes.c_int
            self._libc.inotify_add_watch.argtypes = [
                ctypes.c_int,
                ctypes.c_char_p,
                ctypes.c_uint32,
            ]
            self._libc.inotify_add_watch.restype = ctypes.c_int
            fd = self._libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
            if fd >= 0:
                if self._libc.inotify_add_watch(fd, os.fsencode(self.directory), mask) < 0:
                    os.close(fd)
                else:
                    self.fd = fd
        except OSError:
            self.fd = -1

    @property
    def available(self) -> bool:
        return self.fd >= 0

    def wait(self, timeout: float) -> bool:
        """Return True if an event arrived, False on timeout. Never sleeps."""
        if self.fd < 0:
            return False
        try:
            ready, _, _ = select.select([self.fd], [], [], max(0.0, timeout))
        except OSError as err:
            if err.errno == errno.EINTR:
                return False
            raise
        if not ready:
            return False
        try:
            os.read(self.fd, 65536)  # drain; callers re-read their own state anyway
        except OSError:
            pass
        return True

    def close(self) -> None:
        if self.fd >= 0:
            os.close(self.fd)
            self.fd = -1

    def __enter__(self) -> DirectoryWatch:
        return self

    def __exit__(self, *_exc) -> None:
        self.close()


def process_alive(pid: int) -> bool:
    """True if `pid` exists and is not a zombie. Reads /proc rather than shelling out.

    `pgrep` is deliberately avoided: it embeds the target name in its own command line, so it
    matches itself and trips this repo's guard.
    """
    if pid <= 0:
        return False
    try:
        status = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8", errors="replace")
    except OSError:
        return False
    # "pid (comm) state ..." -- comm can contain spaces and parens, so split on the LAST ')'.
    try:
        return status[status.rindex(")") + 1 :].split()[0] != "Z"
    except (ValueError, IndexError):
        return False


GAME_PROCESS_NAMES = ("eldenring.exe",)


def find_game_pids(names: tuple[str, ...] = GAME_PROCESS_NAMES) -> list[int]:
    """PIDs of the running game, found by reading /proc directly.

    Deliberately not `pgrep`: `pgrep -f eldenring.exe` puts the pattern in its own command
    line and so matches itself, and this repo's guard blocks the bare form outright. Reading
    /proc has neither problem and costs nothing.

    Matches `comm` (truncated to 15 bytes by the kernel, so `eldenring.exe` fits) and, for
    Proton's wrapper processes, the full command line.
    """
    found = []
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            comm = (entry / "comm").read_text(encoding="utf-8", errors="replace").strip()
            if any(comm.lower() == name.lower() for name in names):
                found.append(int(entry.name))
                continue
            cmdline = (entry / "cmdline").read_bytes().replace(b"\0", b" ").decode(
                "utf-8", errors="replace"
            )
        except (OSError, ValueError):
            continue
        lowered = cmdline.lower()
        if any(name.lower() in lowered for name in names):
            found.append(int(entry.name))
    return found


def wait_for_exit(pid: int, timeout: float) -> bool:
    """Block until `pid` exits or `timeout` elapses. Returns True if it exited.

    Uses a pidfd so the wait is edge-triggered on the actual exit, with no polling and no
    requirement that the caller be the process's parent -- which matters here, because the
    reaper deliberately is not.
    """
    if not process_alive(pid):
        return True
    try:
        pidfd = os.pidfd_open(pid, 0)
    except (OSError, AttributeError):
        # No pidfd: fall back to a liveness re-check, which the caller's loop drives.
        return not process_alive(pid)
    try:
        ready, _, _ = select.select([pidfd], [], [], max(0.0, timeout))
        return bool(ready)
    except OSError as err:
        if err.errno == errno.EINTR:
            return not process_alive(pid)
        raise
    finally:
        os.close(pidfd)


@dataclass
class RunState:
    """What a launched run left behind, and what has to be undone when it ends.

    Written before the launch and consumed by whoever gets there first -- the detached
    reaper on a clean exit, or the next invocation's garbage collection if the reaper never
    ran (SIGKILL, reboot, the stale-run sentinel tearing the game down from a hook). The
    cleanup is therefore idempotent by construction: it is a list of paths to remove, and
    removing an absent path is success.
    """

    run_id: str
    pid: int = 0
    profile: str = ""
    remove_paths: list[str] = field(default_factory=list)
    meta: dict = field(default_factory=dict)

    @property
    def directory(self) -> Path:
        return RUN_STATE_ROOT / self.run_id

    @property
    def state_file(self) -> Path:
        return self.directory / "run.json"

    def save(self) -> None:
        self.directory.mkdir(parents=True, exist_ok=True)
        payload = {
            "run_id": self.run_id,
            "pid": self.pid,
            "profile": self.profile,
            "remove_paths": self.remove_paths,
            "meta": self.meta,
        }
        tmp = self.state_file.with_suffix(".tmp")
        tmp.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
        tmp.replace(self.state_file)  # atomic: a torn state file is an uncleanable run

    @classmethod
    def load(cls, state_file: Path) -> RunState | None:
        try:
            payload = json.loads(state_file.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return None
        return cls(
            run_id=payload.get("run_id", state_file.parent.name),
            pid=int(payload.get("pid", 0)),
            profile=payload.get("profile", ""),
            remove_paths=list(payload.get("remove_paths", [])),
            meta=dict(payload.get("meta", {})),
        )

    def cleanup(self) -> list[str]:
        """Remove everything this run staged. Returns what was actually removed."""
        removed: list[str] = []
        for raw in self.remove_paths:
            path = Path(raw)
            try:
                path.unlink()
                removed.append(raw)
            except FileNotFoundError:
                continue
            except OSError:
                continue
        try:
            self.state_file.unlink()
        except OSError:
            pass
        try:
            self.directory.rmdir()
        except OSError:
            pass
        return removed


def all_run_states(root: Path = RUN_STATE_ROOT) -> list[RunState]:
    if not root.is_dir():
        return []
    states = []
    for state_file in sorted(root.glob("*/run.json")):
        state = RunState.load(state_file)
        if state is not None:
            states.append(state)
    return states


def collect_dead_runs(root: Path = RUN_STATE_ROOT) -> list[tuple[str, list[str]]]:
    """Clean up after every run whose process is gone. Returns [(run_id, removed paths)].

    This -- not the reaper -- is what makes cleanup a guarantee. The reaper is the fast path
    and can be killed; this runs at the start of every launch, so a leftover survives at most
    until the next one.
    """
    collected = []
    for state in all_run_states(root):
        if state.pid and process_alive(state.pid):
            continue
        collected.append((state.run_id, state.cleanup()))
    return collected


def selftest() -> int:
    """Exercise every primitive, including that the waits return on the EVENT not the timeout."""
    import subprocess
    import tempfile
    import time

    ok = True

    def check(condition: bool, label: str) -> None:
        nonlocal ok
        if not condition:
            ok = False
        print(("  ok   " if condition else "  FAIL ") + label)

    check(process_alive(os.getpid()), "process_alive sees our own pid")
    check(not process_alive(999_999_999), "process_alive rejects a nonexistent pid")

    with tempfile.TemporaryDirectory() as raw:
        directory = Path(raw)
        with DirectoryWatch(directory) as watch:
            check(watch.available, "inotify watch initialises")
            check(not watch.wait(0.05), "wait() times out cleanly when nothing happens")
            (directory / "evidence.log").write_text("x", encoding="utf-8")
            started = time.monotonic()
            check(watch.wait(5.0), "wait() returns on a real file event")
            check(
                time.monotonic() - started < 1.0,
                "the file wait returned on the EVENT, not by exhausting its cap",
            )

    child = subprocess.Popen(["/bin/true"])
    started = time.monotonic()
    check(wait_for_exit(child.pid, 5.0), "wait_for_exit returns when the child exits")
    check(
        time.monotonic() - started < 2.0,
        "the exit wait returned on the EVENT, not by exhausting its cap",
    )
    child.wait()

    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        staged = root / "staged.me3"
        staged.write_text("profileVersion = \"v1\"\n", encoding="utf-8")

        global RUN_STATE_ROOT
        previous_root, RUN_STATE_ROOT = RUN_STATE_ROOT, root
        try:
            state = RunState(
                run_id="selftest",
                pid=999_999_999,
                profile=str(staged),
                remove_paths=[str(staged)],
            )
            state.save()
            check(state.state_file.is_file(), "run state saves atomically")

            loaded = RunState.load(state.state_file)
            check(
                loaded is not None and loaded.remove_paths == [str(staged)],
                "run state round-trips through disk",
            )

            collected = collect_dead_runs(root)
            check(
                any(run_id == "selftest" for run_id, _ in collected),
                "GC collects a run whose process is gone",
            )
            check(not staged.exists(), "GC actually removed the staged file")
            check(collect_dead_runs(root) == [], "GC is idempotent -- a second pass finds nothing")

            live = RunState(
                run_id="live", pid=os.getpid(), profile="", remove_paths=[str(root / "keep")]
            )
            (root / "keep").write_text("x", encoding="utf-8")
            live.save()
            collect_dead_runs(root)
            check((root / "keep").exists(), "GC leaves a run whose process is still alive alone")
        finally:
            RUN_STATE_ROOT = previous_root

    print("selftest:", "PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    import sys

    sys.exit(selftest())
