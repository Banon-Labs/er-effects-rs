#!/usr/bin/env python3
"""Bring up a Wine-side Frida server in Elden Ring's own prefix, and prove it can see the game.

# Why the server, and not `frida.attach()`

A plain `frida.attach()` on this target injects a Linux bootstrapper into a Windows process and it
segfaults: `frida.NotSupportedError: bootstrapper crashed with signal 11 at offset 0x0`. Measured
again on 2026-09-08 with Frida 17.17.0, so that half of the old finding still holds. The other half
did not: the game survived both attempts (`scripts/er-frida-attach-probe.py` reports the pid alive
before and after), which is why the standing rule against ever touching Frida here was retired.

Running `frida-server.exe` -- the Windows build -- inside the game's own prefix removes the
mismatch entirely. The server is a Windows process talking to a Windows process, so no Linux-side
bootstrapper is ever injected, and the game is reached as an ordinary remote device. Proven
2026-09-08: the server enumerated 15 prefix processes, found `eldenring.exe`, attached, resolved
`ersc.dll` at `0x180000000`, and installed an `Interceptor` on `ersc+0x25850`.

# What this buys

Seamless's session object cannot be found by scanning: the game holds 25,192 objects that match its
static shape and no `ersc.dll` global points at it. Frida answers it in one hook, because
`ersc+0x25850` opens `mov rdi, [rcx + 0x58]` -- so `rcx` at entry is the menu object and `rdi` is
the session, handed over by the game rather than guessed at.

    python3 scripts/er-frida-up.py            # start it, or report the one already running
    uv run --with frida python3 scripts/er-frida-up.py --status   # needs frida in the interpreter
    python3 scripts/er-frida-up.py --stop
    python3 scripts/er-frida-up.py --selftest # no game required

Then, from any script:

    dev = frida.get_device_manager().add_remote_device("127.0.0.1:27042")
    session = dev.attach(dev.enumerate_processes.__self__ and PID)
"""

from __future__ import annotations

import argparse
import os
import pathlib
import shutil
import signal
import socket
import subprocess
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

# The pipeline's sleepless waits: `wait_for_exit` blocks on a pidfd, so every wait below returns on
# the event -- a process dying -- rather than on a clock. See its module docstring.
import er_run_lib

FRIDA_VERSION = "17.17.0"
PORT = 27042
STEAM_APP_ID = "1245620"
# Where the downloaded server is cached between runs. Not in the repo: it is a 65 MB third-party
# binary, the same category as the deobfuscated game images.
CACHE = pathlib.Path(os.environ.get("ER_FRIDA_CACHE", pathlib.Path.home() / ".cache" / "er-frida"))
DOWNLOAD = (
    f"https://github.com/frida/frida/releases/download/{FRIDA_VERSION}/"
    f"frida-server-{FRIDA_VERSION}-windows-x86_64.exe.xz"
)
# One attempt's worth of transfer, and the subprocess bound around it.
#
# `scripts/check-no-timeouts.py` caps every non-game subprocess at 30 seconds and a 65 MB download
# does not fit in one. It does not have to: the fetch below resumes (`curl -C -`), so the cap costs
# a reconnect rather than the download, and progress is measured in bytes on disk instead of in
# elapsed time. The previous 600-second bound was the opposite trade -- a stalled mirror held the
# terminal for ten minutes and then reported nothing but a timeout.
DOWNLOAD_ATTEMPT_SECONDS = 25
DOWNLOAD_SUBPROCESS_TIMEOUT_SECONDS = 30
# `-C -` is what makes the cap affordable: an attempt cut short by `--max-time` is resumed from the
# byte it reached, so the bound costs a reconnect rather than the whole transfer.
DOWNLOAD_CURL_FLAGS = ["-sSL", "--max-time", str(DOWNLOAD_ATTEMPT_SECONDS), "-C", "-"]
# Decompression is a local, bounded job: 65 MB of xz unpacks in a couple of seconds, so the cap here
# is a backstop against a corrupt archive, not a budget.
UNPACK_TIMEOUT_SECONDS = 30
# Enough attempts that a genuinely slow link finishes; an attempt that adds no bytes ends the loop
# immediately, so a dead mirror fails on the first one rather than on the fortieth.
MAX_DOWNLOAD_ATTEMPTS = 40
# The whole wait for the server to open its port, inside one agent-shell command.
SERVER_START_BUDGET_SECONDS = 28.0
# How long one slice blocks on the server's pidfd before the port is probed again.
SERVER_PROBE_SLICE_SECONDS = 1.0
# How long a killed server gets to actually go. One that is still there after this is stuck in the
# kernel, which is a different problem from a stale server.
SERVER_EXIT_WAIT_SECONDS = 5.0


def steam_root() -> pathlib.Path:
    return pathlib.Path(
        os.environ.get("ER_STEAM_DIR", pathlib.Path.home() / ".local/share/Steam")
    )


def prefix() -> pathlib.Path:
    return steam_root() / "steamapps/compatdata" / STEAM_APP_ID / "pfx"


def game_pid() -> int | None:
    """The Linux pid of `eldenring.exe`, or `None`.

    Wine reports the Windows executable name in `comm`; the `exe` symlink points at
    wine64-preloader for every Windows process in the prefix, so `comm` is the discriminator.
    """
    for entry in pathlib.Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            if (entry / "comm").read_text(encoding="utf-8").strip() == "eldenring.exe":
                return int(entry.name)
        except OSError:
            continue
    return None


def container_prefix(pid: int) -> list[str]:
    """The `nsenter` prefix that puts a command inside the game's container, or `[]`.

    # Why this is required rather than optional

    Proton runs the game inside pressure-vessel, so `eldenring.exe` lives in its own mount and user
    namespaces -- measured 2026-09-08: game `mnt:[4026533261]` / `user:[4026533260]` against the
    host's `mnt:[4026531832]` / `user:[4026531837]`. A wineserver is per-namespace, so a
    `frida-server.exe` started beside the container is talking to a different wineserver and sees
    none of the game's processes. It does not fail: it accepts the connection and then never
    answers `enumerate_processes`, which is how a watcher sat silent through an entire run.

    The pid and network namespaces are shared (`pid:[4026531836]`, `net:[4026531833]` on both
    sides), which is what makes this workable: the server can be reached on host localhost once it
    is running in the right mount namespace, and no privilege is needed to enter a namespace this
    user already owns.
    """
    return [
        "nsenter",
        "-t",
        str(pid),
        "-m",
        "-U",
        "--preserve-credentials",
        "--",
    ]


def wine_binary() -> pathlib.Path | None:
    """Proton's own `wine`, so the server runs against the same build the game does."""
    for proton in sorted((steam_root() / "steamapps/common").glob("Proton*")):
        candidate = proton / "files/bin/wine"
        if candidate.is_file():
            return candidate
    return None


def listening() -> bool:
    with socket.socket() as probe:
        probe.settimeout(2)
        try:
            probe.connect(("127.0.0.1", PORT))
            return True
        except OSError:
            return False


def declared_size(url: str) -> int | None:
    """The `Content-Length` the server reports after redirects, or `None`.

    This is the completion predicate for [`fetch_resumable`]. Curl's exit code cannot serve as one:
    a transfer cut short by `--max-time` leaves a partial file behind, and a resumed request that
    the server answers with `416 Range Not Satisfiable` succeeds while appending an error page.
    Bytes on disk against a declared length is the only reading that says the file is whole.
    """
    result = subprocess.run(
        ["curl", "-sSLI", "--max-time", str(DOWNLOAD_ATTEMPT_SECONDS), url],
        capture_output=True,
        check=False,
        timeout=DOWNLOAD_SUBPROCESS_TIMEOUT_SECONDS,
    )
    size = None
    for line in result.stdout.decode("utf-8", errors="replace").splitlines():
        name, _, value = line.partition(":")
        # Last one wins: a redirect chain prints one header block per hop, and the file's length is
        # the one the final hop declares.
        if name.strip().lower() == "content-length" and value.strip().isdigit():
            size = int(value.strip())
    return size


def fetch_resumable(url: str, destination: pathlib.Path) -> None:
    """Fetch `url` into `destination` in bounded attempts, resuming rather than restarting.

    Each attempt is its own subprocess under the 30-second cap, and the loop advances on an
    observation -- the file grew -- rather than on a timer. An attempt that adds nothing stops it at
    once with what was actually seen, so an unreachable mirror is a fast, legible failure instead of
    a ten-minute hang.
    """
    expected = declared_size(url)
    for attempt in range(1, MAX_DOWNLOAD_ATTEMPTS + 1):
        have = destination.stat().st_size if destination.exists() else 0
        if expected is not None and have >= expected:
            return
        result = subprocess.run(
            ["curl", *DOWNLOAD_CURL_FLAGS, "-o", str(destination), url],
            check=False,
            timeout=DOWNLOAD_SUBPROCESS_TIMEOUT_SECONDS,
        )
        now = destination.stat().st_size if destination.exists() else 0
        if expected is None and result.returncode == 0 and now > 0:
            return
        if now <= have:
            raise SystemExit(
                f"download made no progress on attempt {attempt}: {now} bytes of "
                f"{expected if expected is not None else 'an undeclared total'} from {url} "
                f"(curl exit {result.returncode}). Fetch it by hand into {destination.parent} if "
                "the link is the problem."
            )
        print(f"  {now}/{expected if expected is not None else '?'} bytes", flush=True)
    raise SystemExit(
        f"download did not finish in {MAX_DOWNLOAD_ATTEMPTS} attempts; {destination} holds "
        f"{destination.stat().st_size if destination.exists() else 0} bytes and will be resumed by "
        "the next run"
    )


def ensure_binary() -> pathlib.Path:
    CACHE.mkdir(parents=True, exist_ok=True)
    server = CACHE / f"frida-server-{FRIDA_VERSION}.exe"
    if server.is_file():
        return server
    packed = CACHE / "frida-server.exe.xz"
    print(f"fetching {DOWNLOAD}", flush=True)
    fetch_resumable(DOWNLOAD, packed)
    subprocess.run(["unxz", "-f", str(packed)], check=True, timeout=UNPACK_TIMEOUT_SECONDS)
    shutil.move(str(CACHE / "frida-server.exe"), str(server))
    return server


def start(force: bool = False) -> int:
    # An open port is not proof of a usable server. Measured 2026-09-08: a server started before
    # `scripts/er-teardown.py` killed the prefix keeps its listening socket, accepts the connection,
    # and then hangs forever inside `enumerate_processes` -- so a watcher started against it sits
    # mute rather than failing, which is exactly how a run reached the player with nothing attached.
    # The server's view is per-wineserver, so a teardown invalidates it and it has to be replaced.
    if force:
        # `--force` has to stop the old server, not merely skip the reuse check. Without this the
        # replacement starts, fails with `Unable to start: Error binding to address 127.0.0.1:27042`
        # because the previous one still holds the port, and dies -- leaving the stale server
        # answering. Measured 2026-09-08: after a relaunch the old server was still alive in the
        # previous container's namespace (mnt:[4026533261]) while the game had moved to a new one
        # (mnt:[4026533335]), so every enumerate hung and `--force` appeared to do nothing.
        stop()
    elif listening():
        if server_sees_the_prefix():
            print(f"frida-server already listening on 127.0.0.1:{PORT} and answering")
            return 0
        print("frida-server is listening but not answering; replacing it", flush=True)
        stop()
    wine = wine_binary()
    if wine is None:
        print("no Proton wine binary found under Steam", file=sys.stderr)
        return 1
    if not prefix().is_dir():
        print(f"no wine prefix at {prefix()}", file=sys.stderr)
        return 1
    server = ensure_binary()
    staged = prefix() / "drive_c" / server.name
    shutil.copyfile(server, staged)
    env = dict(os.environ)
    env["WINEPREFIX"] = str(prefix())
    # The server is a console app that needs neither; suppressing them keeps the log readable.
    env["WINEDLLOVERRIDES"] = "mscoree=d;mshtml=d"
    log = CACHE / "frida-server.log"
    pid = game_pid()
    command = [str(wine), str(staged)]
    if pid is not None:
        command = container_prefix(pid) + command
        print(f"starting frida-server inside the game's container (pid {pid})", flush=True)
    else:
        print("no eldenring.exe yet; starting frida-server on the host namespace", flush=True)
    with open(log, "wb") as handle:
        server = subprocess.Popen(
            command,
            env=env,
            stdout=handle,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
    if wait_for_port(server):
        print(f"frida-server up on 127.0.0.1:{PORT} (log {log})")
        return 0
    print(f"frida-server did not open {PORT}; see {log}", file=sys.stderr)
    return 1


def wait_for_port(server: subprocess.Popen) -> bool:
    """Wait for the server to open `PORT`, or to die trying.

    Readiness is the connect probe, because a listening socket is the only thing that proves the
    server is up. What this does not do is pace the probes with a delay: between them it blocks on
    the server's own pidfd, so the documented failure -- an immediate exit with `Error binding to
    address 127.0.0.1:27042` because a previous server still holds the port -- ends the wait the
    instant it happens instead of thirty seconds later.
    """
    deadline = time.monotonic() + SERVER_START_BUDGET_SECONDS
    while True:
        if listening():
            return True
        if server.poll() is not None:
            print(
                f"frida-server exited with {server.returncode} before opening {PORT}",
                file=sys.stderr,
            )
            return False
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return False
        er_run_lib.wait_for_exit(server.pid, min(SERVER_PROBE_SLICE_SECONDS, remaining))


def server_sees_the_prefix(timeout_seconds: float = 6.0) -> bool:
    """Whether the server can still enumerate its prefix, within a bound.

    `enumerate_processes` has no timeout of its own and blocks indefinitely against a server whose
    wineserver is gone, so the call is made on a daemon thread and abandoned if it does not answer.
    Abandoning it is safe: the process is about to be replaced either way.
    """
    import threading

    answered: list[bool] = []

    def ask() -> None:
        try:
            import frida

            dev = frida.get_device_manager().add_remote_device(f"127.0.0.1:{PORT}")
            dev.enumerate_processes()
            answered.append(True)
        except Exception:
            answered.append(False)

    worker = threading.Thread(target=ask, daemon=True)
    worker.start()
    worker.join(timeout_seconds)
    return bool(answered) and answered[0]


def status() -> int:
    up = listening()
    print(f"127.0.0.1:{PORT} {'OPEN' if up else 'closed'}")
    if not up:
        return 1
    try:
        import frida
    except ImportError:
        print("frida python not importable here; run under `uv run --with frida`")
        return 0
    device = frida.get_device_manager().add_remote_device(f"127.0.0.1:{PORT}")
    processes = device.enumerate_processes()
    print(f"{len(processes)} process(es) visible in the prefix")
    for process in processes:
        if "eldenring" in process.name.lower():
            print(f"  eldenring.exe is windows pid {process.pid}")
    return 0


def frida_server_pids() -> list[int]:
    """Every frida-server, matched on `comm` rather than the full command line.

    `pkill -f frida-server` matches any process whose argv mentions the name -- including the shell
    that invoked this script, which killed a session on 2026-09-08. Reading `/proc/*/comm` and
    signalling by pid cannot make that mistake.
    """
    pids = []
    for entry in pathlib.Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        pid = int(entry.name)
        if pid == os.getpid():
            continue
        try:
            if (entry / "comm").read_text(encoding="utf-8").strip().startswith("frida-server"):
                pids.append(pid)
        except OSError:
            continue
    return pids


def stop() -> int:
    """Kill every frida-server and wait for each one to actually go.

    The wait is a pidfd, not a delay. What a caller needs from this function is that the port has
    been released, and the event that releases it is the holder dying -- so `--force` starting its
    replacement is now gated on the previous server being gone rather than on a second having
    passed. That distinction was measured on 2026-09-08: a replacement started too early fails with
    `Error binding to address 127.0.0.1:27042` and dies, leaving the stale server answering, which
    is how `--force` came to look like it did nothing at all.
    """
    killed = []
    for pid in frida_server_pids():
        try:
            os.kill(pid, signal.SIGKILL)
            killed.append(pid)
        except OSError:
            continue
    for pid in killed:
        er_run_lib.wait_for_exit(pid, SERVER_EXIT_WAIT_SECONDS)
    print(f"stopped {killed or 'nothing'}; 127.0.0.1:{PORT} {'still OPEN' if listening() else 'closed'}")
    return 0


def selftest() -> int:
    source = pathlib.Path(__file__).read_text(encoding="utf-8")
    # The `--force` branch, read structurally rather than by quoting the comment inside it. The
    # quoted form asserted on prose and broke the day the prose was reworded, which is the one thing
    # a check on a file that is already correct must not do.
    force_branch = source.split("\n    if force:", 1)[-1].split("elif listening():", 1)[0]
    checks = [
        ("a Proton wine binary is resolvable", wine_binary() is not None),
        ("the game's wine prefix exists", prefix().is_dir()),
        ("the download url names the pinned version", FRIDA_VERSION in DOWNLOAD),
        ("the cache directory is user-owned, not a repo path", "er-mods-rs" not in str(CACHE)),
        ("--force stops the old server before starting a new one", "stop()" in force_branch),
        (
            "the server is stopped by comm, never a broad pkill -f pattern",
            # Built from pieces so the check does not match its own source text -- a literal here
            # made the assertion fail against a file that was already correct.
            ("sub" + "process.run([\"pk" + "ill\"")
            not in pathlib.Path(__file__).read_text(encoding="utf-8"),
        ),
        (
            "the server is started inside the game's container when one exists",
            "container_prefix(" in pathlib.Path(__file__).read_text(encoding="utf-8"),
        ),
        (
            "an open port alone is not treated as a working server",
            "server_sees_the_prefix()" in pathlib.Path(__file__).read_text(encoding="utf-8"),
        ),
        (
            "every wait is an event wait, never a delay",
            # Built from pieces so the assertion does not match its own source text.
            ("ti" + "me.sl" + "eep(")
            not in pathlib.Path(__file__).read_text(encoding="utf-8"),
        ),
        (
            "a killed server is waited out on its pidfd",
            "er_run_lib.wait_for_exit(pid, SERVER_EXIT_WAIT_SECONDS)"
            in pathlib.Path(__file__).read_text(encoding="utf-8"),
        ),
        (
            "the download is resumable, so each attempt fits the 30-second subprocess cap",
            "-C" in DOWNLOAD_CURL_FLAGS
            and DOWNLOAD_SUBPROCESS_TIMEOUT_SECONDS <= 30
            and DOWNLOAD_ATTEMPT_SECONDS < DOWNLOAD_SUBPROCESS_TIMEOUT_SECONDS,
        ),
        (
            "the fetch completes on a size, not on curl's exit code",
            "declared_size(" in pathlib.Path(__file__).read_text(encoding="utf-8"),
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
    parser.add_argument("--status", action="store_true")
    parser.add_argument("--stop", action="store_true")
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--force", action="store_true", help="replace a running server outright")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if args.status:
        return status()
    if args.stop:
        return stop()
    return start(force=args.force)


if __name__ == "__main__":
    raise SystemExit(main())
