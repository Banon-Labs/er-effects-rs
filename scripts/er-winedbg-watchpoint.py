#!/usr/bin/env python3
"""Hardware write watchpoint on a field of a live Wine/Proton `eldenring.exe`.

Why this exists
---------------
`scripts/er-live-fields.py` answers "what is this field NOW" by reading /proc/<pid>/mem.
It cannot answer "WHICH INSTRUCTION wrote it", and neither can an in-process function hook
when the writer set has already been enumerated byte-complete and every member was silent
at the moment of interest (bd `savestate-writer-set-complete-and-silent-at-the-wedge-2026-08-31`).
The remaining instrument is a data watchpoint, and AGENTS.md names exactly one sanctioned
transport for a Wine target: the `linux-x86-debug` toolkit's `winedbg --gdb` attach.
Never `frida.attach()` here -- it segfaults inside eldenring.exe and kills the game.

Why `--no-start` rather than the toolkit'S `tracebreakpoint`
-----------------------------------------------------------
`tracebreakpoint` only emits `break *<addr>` (a code breakpoint). The question here is a data
write to an address that is not known to be written by any code we can name, so the tool's
own gdb-script builder cannot express it. This drives the same transport with a different
script: `winedbg --gdb --no-start --port N <windows-pid>` opens a gdb stub on localhost and
a host gdb connects to it. Two things fall out of that split, both of which matter here:

  * gdb does not have to exist inside the Steam Linux Runtime container (it does not), and
  * the host's own gdb (with `watch`) drives the session.

Measured container facts (2026-08-31, Proton Experimental + SteamLinuxRuntime_4)
-------------------------------------------------------------------------------
  * mount namespace is not shared with the host, but the PID and net namespaces are.
    So localhost TCP crosses the boundary and `--port` works.
  * the prefix's wineserver socket is `/tmp/.wine-<uid>/server-<dev>-<ino>/socket`, derived
    from the WINEPREFIX directory's st_dev/st_ino, and the host can see it: /tmp is shared.
    That is what lets a host-launched Proton `wine` join the running prefix.
  * the game image is at 0x140000000 (WINEPRELOADRESERVE=140000000-145e0a000), so a 1.17
    deobf VA needs no runtime translation.

The Wine Windows pid (what `winedbg --gdb` wants -- not the unix pid) is printed by me3 as
`attaching to process pid=<N>` in its launch log; `--me3-log` parses it.

Positive control, always
------------------------
A watchpoint that silently fails to arm looks exactly like a field nobody writes. Hardware
data breakpoints are known to be silently dropped on some Wine configurations here (bd
`wine-proton-no-hardware-data-breakpoints-2026`: DR0/DR7 set through SetThreadContext on 85
threads, zero traps, while the field provably changed). So this refuses to report "no writer"
on its own: it records whether gdb called the watchpoint hardware, and the caller must
compare the hit count against an independent in-process count of writes over the same window.

Usage:
    scripts/er-winedbg-watchpoint.py --mode probe  --out DIR      # arm, report, detach
    scripts/er-winedbg-watchpoint.py --mode arm --out DIR --deadline-seconds 200
    scripts/er-winedbg-watchpoint.py --selftest                   # no game needed
"""

from __future__ import annotations

import argparse
import json
import os
import re
import selectors
import shlex
import socket
import struct
import subprocess
import sys
import time
from pathlib import Path

PROC = "/proc"
IMAGE_BASE = 0x140000000
GAME_MAN_SINGLETON_VA = 0x143D6D988  # 1.17; 1.16.2 had it at 0x143d69918
SAVE_STATE_OFFSET = 0xB80
DEFAULT_PROTON = "Proton - Experimental"

HIT = "--WP-HIT--"
HITEND = "--WP-HITEND--"
ARMED = "--WP-ARMED--"
CONNECTED = "--WP-CONNECTED--"


def find_pid(name: str) -> int | None:
    """Resolve a process by `comm`. Not pgrep: the repo guard blocks it and it
    false-negatives on this box. `comm` is kernel-truncated to 15 chars."""
    want = name.lower()
    for entry in os.listdir(PROC):
        if not entry.isdigit():
            continue
        try:
            with open(f"{PROC}/{entry}/comm", encoding="utf-8", errors="replace") as fh:
                comm = fh.read().strip().lower()
        except OSError:
            continue
        if comm and (comm == want or want.startswith(comm) or comm.startswith(want[:15])):
            return int(entry)
    return None


def read_mem(pid: int, addr: int, size: int) -> bytes | None:
    try:
        with open(f"{PROC}/{pid}/mem", "rb", 0) as fh:
            fh.seek(addr)
            return fh.read(size)
    except (OSError, ValueError):
        return None


def read_environ(pid: int) -> dict[str, str]:
    try:
        raw = Path(f"{PROC}/{pid}/environ").read_bytes()
    except OSError:
        return {}
    out: dict[str, str] = {}
    for kv in raw.split(b"\0"):
        if b"=" in kv:
            k, v = kv.split(b"=", 1)
            out[k.decode(errors="replace")] = v.decode(errors="replace")
    return out


def windows_pid_from_me3_log(path: Path) -> int | None:
    """me3 logs `attaching to process pid=<N>` with the Windows pid it created."""
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None
    # The log is ANSI-coloured and the escape sequences themselves contain digits
    # (`\x1b[3m`), so a `\D`-tolerant pattern matches the colour code instead of the pid.
    # Strip the escapes first and match the plain text.
    plain = re.sub(r"\x1b\[[0-9;]*[A-Za-z]", "", text)
    matches = re.findall(r"attaching to process\s*pid\s*=\s*(\d+)", plain)
    return int(matches[-1]) if matches else None


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


def gdb_script(addr: int, mode: str, max_hits: int, port: int, control_hits: int = 3) -> str:
    """The whole session as one gdb command file. `commands` blocks run per hit.

    Symbol loading is the enemy. winedbg's stub advertises `exec-file`, so on connect gdb
    reads `eldenring.exe` (>100 MB) plus every loaded Wine DLL off the local filesystem to
    build minimal symbol tables -- and the game is frozen for the whole of it. Measured
    2026-08-31: a session spent its entire 220 s budget there and never reached the `watch`
    command, which reads exactly like a field nobody writes. `auto-solib-add off` plus a
    sysroot that resolves to nothing skips all of it. Nothing here needs symbols: the answer
    wanted is a raw $rip, which is then resolved offline against the 1.17 deobf image.

    The control shares the mechanism. `awatch` (read or write) on the same address uses the
    same debug register and the same trap delivery as the write watch, and saveState is read
    every frame (`IsSaveStateIdle` from the in-map MoveMapStep tick). So a few control hits
    prove the hardware path is live before the write watch's silence is allowed to mean
    anything -- exactly the failure mode of bd `wine-proton-no-hardware-data-breakpoints-2026`,
    where DR0/DR7 were accepted on 85 threads and no trap was ever delivered.
    """
    lines = [
        "set pagination off",
        "set confirm off",
        "set height 0",
        "set width 0",
        "set print elements 0",
        "set auto-solib-add off",
        "set sysroot /nonexistent-er-watchpoint-sysroot",
        "set debuginfod enabled off",
        "set architecture i386:x86-64",
        "set can-use-hw-watchpoints 1",
        # Without this, WINEDBG crashes and takes the session with it. gdb's default is to
        # remove every breakpoint at each internal stop and re-insert before resuming. This
        # target creates and destroys threads constantly, so a stop lands right after a
        # thread exit, and winedbg's `be_x86_64_remove_Xpoint` then clears DR7 through that
        # dead thread's context -- `and %rax,0x70(%r8)` with r8 invalid. Measured 2026-08-31:
        # `winedbg: Internal crash at 000000014001E262`, one `[Thread 1660 exited]` line after
        # arming, and the remote closed before a single hit. Keeping the watchpoints inserted
        # means the remove path only runs once, at detach.
        "set breakpoint always-inserted on",
        "set print thread-events off",
        f"target remote 127.0.0.1:{port}",
        f'printf "\\n{CONNECTED}\\n"',
        "info threads",
    ]
    # The control watchpoint is its own session. Retiring it mid-session is not an option:
    # `delete`/`disable` is a remove, and a remove is the crash (measured run ...-k, three
    # control hits then `winedbg: Internal crash at 000000014001E262` on the delete, leaving
    # the game suspended for good). Leaving it armed is not an option either -- saveState is
    # read every frame, so the game would stop every frame. So: control_hits > 0 means a
    # short control session that is expected to end badly, and control_hits == 0 means the
    # measurement session, which never removes anything until it is done.
    if mode == "attach":
        # Control for the instrument ITSELF: connect and resume with no watchpoint at all.
        # Every armed run so far ended `TELEMETRY_FROZEN_HUNG`, but so did some runs with no
        # debugger, so "armed" and "attached" have to be separated before either is blamed.
        lines += [f'printf "\\n{ARMED}\\n"', "continue"]
        return "\n".join(lines) + "\n"
    write_bp = 1
    if control_hits > 0:
        lines.append(f"awatch *(int*)0x{addr:x}")
        write_bp = 2
    lines.append(f"watch *(int*)0x{addr:x}")
    lines.append(f'printf "\\n{ARMED}\\n"')
    lines.append("info watchpoints")
    if mode == "probe":
        lines += ["detach", "quit"]
        return "\n".join(lines) + "\n"
    if control_hits > 0:
        lines += [
            "set $__ctrl = 0",
            "commands 1",
            "set $__ctrl = $__ctrl + 1",
            'printf "\\n--WP-CTRL-HIT-- n=%d rip=%p\\n", $__ctrl, $rip',
            f"if $__ctrl >= {control_hits}",
            '  printf "\\n--WP-CTRL-DONE--\\n"',
            "  detach",
            "  quit",
            "end",
            "continue",
            "end",
        ]
    lines += [
        "set $__hits = 0",
        f"commands {write_bp}",
        "set $__hits = $__hits + 1",
        f'printf "\\n{HIT} n=%d rip=%p\\n", $__hits, $rip',
        # Wall clock per hit, to align a hit with the DLL's own +NNNNms telemetry timeline.
        # gdb's in-process Python, not `shell date`: the game is stopped for the whole of
        # this block, and a fork+exec per hit is the most expensive thing in it.
        'python import time; print("--WP-CLOCK--\\n%.3f" % time.time())',
        "info registers rip rsp rbp rax rbx rcx rdx rsi rdi r8 r9 r10 r11 r12 r13 r14 r15",
        'printf "\\n--WP-STACK--\\n"',
        # Eight words is enough to spot return addresses in 0x140000000..0x143000000; every
        # extra word is another stub round trip with the game frozen.
        "x/8gx $rsp",
        'printf "\\n--WP-VAL--\\n"',
        f"p/x *(int*)0x{addr:x}",
        # `info threads` here would cost a stub round trip per thread, and this target runs
        # ~85 of them -- seconds of frozen game per hit. `$_thread` is already local.
        'printf "\\n--WP-THREAD-- %d\\n", $_thread',
        f'printf "\\n{HITEND}\\n"',
        f"if $__hits >= {max_hits}",
        '  printf "\\n--WP-MAXHITS--\\n"',
        "  detach",
        "  quit",
        "end",
        "continue",
        "end",
        "continue",
    ]
    return "\n".join(lines) + "\n"


def proton_wine(steam_dir: Path, proton_name: str) -> Path:
    return steam_dir / "steamapps" / "common" / proton_name / "files" / "bin" / "wine"


def wait_for_proxy_port(proc: subprocess.Popen, port: int, deadline: float) -> bool:
    """Wait until the winedbg gdb stub accepts a connection on the port we chose.

    The obvious readiness signal -- winedbg's own `target remote localhost:%d` line -- is a
    trap, and it cost a run on 2026-08-31. winedbg writes it to a pipe, so libc switches from
    line buffering to block buffering and the line sits unflushed. Meanwhile the target is
    already suspended waiting for gdb, so no further output is produced to flush it: the game
    is frozen, the reader is blocked, and neither side moves. The port number is not news --
    this process passed `--port` on the command line -- so the sound signal is the stub's
    socket reaching listen. That is read out of /proc/net/tcp rather than probed by
    connecting: the stub accepts exactly one client, so a probe connect would consume the
    accept and gdb would arrive to a closed door. The lookup is instant, so the selector
    call, not the lookup, is what paces the loop.
    """
    sel = selectors.DefaultSelector()
    try:
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                return False
            if tcp_port_listening(port):
                return True
            sel.select(timeout=0.25)
    finally:
        sel.close()
    return False


def tcp_port_listening(port: int) -> bool:
    """True when a socket is in listen (state 0A) on `port`, per /proc/net/tcp*."""
    for table in ("/proc/net/tcp", "/proc/net/tcp6"):
        try:
            with open(table, encoding="utf-8", errors="replace") as fh:
                next(fh, None)
                for line in fh:
                    cols = line.split()
                    if len(cols) < 4 or cols[3] != "0A":
                        continue
                    if int(cols[1].rsplit(":", 1)[1], 16) == port:
                        return True
        except OSError:
            continue
    return False


def drain_proxy(proc: subprocess.Popen, sink: list[str]) -> None:
    """Non-blocking scrape of whatever the proxy has flushed so far."""
    if proc.stdout is None:
        return
    sel = selectors.DefaultSelector()
    sel.register(proc.stdout, selectors.EVENT_READ)
    try:
        while sel.select(timeout=0):
            line = proc.stdout.readline()
            if not line:
                return
            sink.append(line.rstrip("\n"))
    finally:
        sel.unregister(proc.stdout)
        sel.close()


def telemetry_true(path: Path | None, key: str, minimum: float = 1.0) -> bool:
    """True when the DLL's live telemetry reports `key` at or above `minimum`.

    Tolerates a torn read: the file is rewritten continuously, so a parse failure means
    "not yet", not an error. A bool True counts as 1 so a flag oracle works with the
    default minimum, and the sentinel -1 never counts."""
    if path is None:
        return True
    try:
        data = json.loads(path.read_text(encoding="utf-8", errors="replace"))
    except (OSError, ValueError):
        return False
    value = data.get(key)
    if isinstance(value, bool):
        value = 1 if value else 0
    if not isinstance(value, (int, float)):
        return False
    return value != -1 and value >= minimum


def wait_for_target(me3_log: Path | None, seconds: float,
                    telemetry: Path | None = None,
                    telemetry_key: str = "",
                    telemetry_min: float = 1.0) -> tuple[int | None, int | None, int | None]:
    """Wait for (unix pid, windows pid, GameMan) and, optionally, an in-world semaphore.

    Every wait here is a real readiness signal -- a process appearing in /proc, a line
    appearing in me3's log, a singleton pointer becoming non-NULL, an oracle flipping in the
    DLL's telemetry -- never a fixed delay. `selectors.select` on an empty set is the pacing
    primitive so this does not spin a core.

    Why the SEMAPHORE. Attaching a debugger stops the process, and doing that during the
    boot/asset-load phase wedges this game: runs j/k/l/m all attached inside the first ~20 s
    and every one of them ended `TELEMETRY_FROZEN_HUNG` with the player never reaching the
    world, while runs b and d attached after load1 was up and both survived to load3. The
    wedge under investigation happens at +70..100 s, well after `oracle_player_present`, so
    waiting for it costs no coverage.
    """
    sel = selectors.DefaultSelector()
    deadline = time.monotonic() + seconds
    while True:
        pid = find_pid("eldenring.exe")
        winpid = windows_pid_from_me3_log(me3_log) if me3_log else None
        game_man = None
        if pid:
            raw = read_mem(pid, GAME_MAN_SINGLETON_VA, 8)
            if raw and len(raw) == 8:
                value = struct.unpack("<Q", raw)[0]
                game_man = value or None
        in_world = telemetry_true(telemetry, telemetry_key, telemetry_min) if telemetry_key else True
        if pid and game_man and in_world and (winpid or me3_log is None):
            return pid, winpid, game_man
        if time.monotonic() >= deadline:
            return pid, winpid, game_man
        sel.select(timeout=0.25)


def parse_hits(text: str) -> list[dict[str, object]]:
    """Split the gdb transcript into one record per watchpoint trigger."""
    hits: list[dict[str, object]] = []
    blocks = text.split(HIT)
    for block in blocks[1:]:
        body = block.split(HITEND)[0]
        head = body.splitlines()[0] if body else ""
        rec: dict[str, object] = {}
        m = re.search(r"n=(\d+)\s+rip=(0x[0-9a-fA-F]+)", head)
        if m:
            rec["n"] = int(m.group(1))
            rec["rip"] = m.group(2)
        regs: dict[str, str] = {}
        for rm in re.finditer(r"^(r[a-z0-9]{1,3}|rip)\s+(0x[0-9a-fA-F]+)", body, re.M):
            regs[rm.group(1)] = rm.group(2)
        rec["registers"] = regs
        stack = re.findall(r"0x[0-9a-fA-F]+:\s+((?:0x[0-9a-fA-F]+\s*)+)", body)
        words: list[str] = []
        for chunk in stack:
            words.extend(chunk.split())
        rec["stack"] = words[:24]
        vm = re.search(r"\$\d+\s*=\s*(0x[0-9a-fA-F]+)", body)
        if vm:
            rec["value_after"] = vm.group(1)
        cm = re.search(r"--WP-CLOCK--\s*\n\s*(\d+\.\d+)", body)
        if cm:
            rec["clock"] = float(cm.group(1))
        tm = re.search(r"--WP-THREAD--\s*(\d+)", body)
        if tm:
            rec["gdb_thread"] = int(tm.group(1))
        hits.append(rec)
    # gdb prints Old/New value before the commands block; pair them positionally.
    pairs = re.findall(r"Old value = (-?\d+)\s*\n\s*New value = (-?\d+)", text)
    for i, (old, new) in enumerate(pairs):
        if i < len(hits):
            hits[i]["old"] = int(old)
            hits[i]["new"] = int(new)
    return hits


def selftest() -> int:
    """No game, no wine: prove the pure functions the live path depends on."""
    ok = True
    script = gdb_script(0x8C06CC00, "arm", 40, 1234, control_hits=0)
    for needed in ("watch *(int*)0x8c06cc00", "target remote 127.0.0.1:1234",
                   "set can-use-hw-watchpoints 1", "continue"):
        if needed not in script:
            print(f"selftest FAIL: gdb script missing {needed!r}")
            ok = False
    probe = gdb_script(0x10, "probe", 1, 1)
    if "commands" in probe or "detach" not in probe:
        print("selftest FAIL: probe script must arm-and-detach without a commands block")
        ok = False
    sample = (
        "Hardware watchpoint 1: *(int*)0x8c06cc00\n"
        "Old value = 1\nNew value = 0\n"
        f"\n{HIT} n=1 rip=0x14067ac9e\n"
        "rip            0x14067ac9e         0x14067ac9e\n"
        "rax            0x8c06c080          2349285504\n"
        "--WP-STACK--\n"
        "0x7ffe0000:\t0x000000014067b1cf\t0x0000000000000000\n"
        "--WP-VAL--\n$1 = 0x0\n"
        f"{HITEND}\n"
    )
    hits = parse_hits(sample)
    if len(hits) != 1:
        print(f"selftest FAIL: expected 1 hit, parsed {len(hits)}")
        ok = False
    else:
        h = hits[0]
        for key, want in (("rip", "0x14067ac9e"), ("old", 1), ("new", 0), ("value_after", "0x0")):
            if h.get(key) != want:
                print(f"selftest FAIL: hit[{key}] = {h.get(key)!r}, want {want!r}")
                ok = False
        if h.get("registers", {}).get("rax") != "0x8c06c080":
            print(f"selftest FAIL: registers not parsed: {h.get('registers')}")
            ok = False
        if "0x000000014067b1cf" not in h.get("stack", []):
            print(f"selftest FAIL: stack words not parsed: {h.get('stack')}")
            ok = False
    log = ("INFO \x1b[1mrun\x1b[0m:\x1b[1mattach\x1b[0m: me3_launcher::game: "
           "attaching to process \x1b[3mpid\x1b[0m\x1b[2m=\x1b[0m388\n")
    tmp = Path(os.environ.get("TMPDIR", "/tmp")) / f"er-wp-selftest-{os.getpid()}.log"
    tmp.write_text(log, encoding="utf-8")
    try:
        got = windows_pid_from_me3_log(tmp)
    finally:
        tmp.unlink(missing_ok=True)
    if got != 388:
        print(f"selftest FAIL: windows pid parsed as {got}, want 388")
        ok = False
    # The listen detector is the readiness signal the whole live path hangs off, and its
    # failure mode (never fires) is indistinguishable from "the proxy did not start".
    with socket.socket() as srv:
        srv.bind(("127.0.0.1", 0))
        srv.listen(1)
        listen_port = int(srv.getsockname()[1])
        if not tcp_port_listening(listen_port):
            print(f"selftest FAIL: tcp_port_listening missed a real LISTEN on {listen_port}")
            ok = False
    if tcp_port_listening(listen_port):
        print(f"selftest FAIL: tcp_port_listening reported a closed port {listen_port} as LISTEN")
        ok = False
    ctrl_script = gdb_script(0x10, "arm", 4, 1, control_hits=2)
    for needed in ("awatch *(int*)0x10", "commands 1", "commands 2"):
        if needed not in ctrl_script:
            print(f"selftest FAIL: control script missing {needed!r}")
            ok = False
    # A remove is the crash, so neither script may contain one before it is finished.
    for name, text in (("control", ctrl_script), ("measurement", script)):
        for banned in ("delete ", "disable "):
            if banned in text:
                print(f"selftest FAIL: {name} script contains {banned!r} -- removing a hardware "
                      f"watchpoint crashes winedbg and leaves the game suspended")
                ok = False
    if "set breakpoint always-inserted on" not in script:
        print("selftest FAIL: always-inserted off -- gdb will remove/reinsert at every stop")
        ok = False
    # With no control watchpoint the write watch is breakpoint 1, and a `commands 2` there
    # would attach the capture to a breakpoint that does not exist -- a silent zero-hit run.
    if "commands 1" not in script or "commands 2" in script:
        print("selftest FAIL: measurement script must drive breakpoint 1, not 2")
        ok = False
    if "set auto-solib-add off" not in ctrl_script:
        print("selftest FAIL: symbol loading not disabled -- the game freezes for the whole load")
        ok = False
    print("selftest PASS" if ok else "selftest FAILED")
    return 0 if ok else 1


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--mode", choices=("probe", "arm", "attach"), default="probe")
    ap.add_argument("--out", help="directory for evidence artifacts")
    ap.add_argument("--pid", type=int, help="unix pid of eldenring.exe (default: discover)")
    ap.add_argument("--winpid", type=int, help="Wine WINDOWS pid (default: from --me3-log)")
    ap.add_argument("--me3-log", help="me3-launch.log to parse the windows pid from")
    ap.add_argument("--addr", help="absolute VA to watch (default: GameMan + --field)")
    ap.add_argument("--field", default=hex(SAVE_STATE_OFFSET),
                    help="offset into GameMan to watch (default 0xb80 = saveState)")
    ap.add_argument("--max-hits", type=int, default=64)
    ap.add_argument("--control-hits", type=int, default=0,
                   help="run a SHORT awatch control session instead of the measurement: it "
                        "proves hardware traps are delivered on this target, and it is "
                        "expected to end by crashing winedbg on detach (which suspends the "
                        "game), so never mix it into a measurement run")
    ap.add_argument("--deadline-seconds", type=float, default=25.0,
                    help="hard wall for the whole session; the primary stop is the game exiting")
    ap.add_argument("--proton", default=DEFAULT_PROTON)
    ap.add_argument("--wait-seconds", type=float, default=0.0,
                   help="wait this long for the game, its me3 windows pid and a non-NULL GameMan "
                        "before attaching, so the tool can be launched alongside the run rather "
                        "than after it")
    ap.add_argument("--telemetry", help="er-quickload-telemetry.json to gate the attach on")
    ap.add_argument("--wait-oracle", default="oracle_player_present",
                   help="telemetry oracle that must be set before attaching (attaching during "
                        "boot/asset load wedges the game -- see wait_for_target)")
    ap.add_argument("--wait-oracle-min", type=float, default=1.0,
                   help="value the --wait-oracle must reach before attaching")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        return selftest()
    if not args.out:
        print("er-winedbg-watchpoint: --out DIR is required for a live session", file=sys.stderr)
        return 2

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    report: dict[str, object] = {"mode": args.mode, "started": time.time()}

    waited_winpid = None
    if args.wait_seconds > 0:
        me3_log = Path(args.me3_log) if args.me3_log else None
        telemetry = Path(args.telemetry) if args.telemetry else None
        pid, waited_winpid, _gm = wait_for_target(
            me3_log, args.wait_seconds, telemetry,
            args.wait_oracle if telemetry else "", args.wait_oracle_min)
        report["waited_seconds"] = args.wait_seconds
        report["wait_oracle"] = args.wait_oracle if telemetry else None
        report["wait_oracle_min"] = args.wait_oracle_min
        report["wait_oracle_satisfied"] = telemetry_true(
            telemetry, args.wait_oracle, args.wait_oracle_min) if telemetry else None
    else:
        pid = args.pid or find_pid("eldenring.exe")
    if not pid:
        report["error"] = "no live eldenring.exe"
        (out / "watchpoint.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
        print("er-winedbg-watchpoint: no live eldenring.exe", file=sys.stderr)
        return 3
    report["pid"] = pid

    winpid = args.winpid if args.winpid is not None else waited_winpid
    if winpid is None and args.me3_log:
        winpid = windows_pid_from_me3_log(Path(args.me3_log))
    if winpid is None:
        report["error"] = "windows pid unknown (pass --winpid or --me3-log)"
        (out / "watchpoint.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
        print("er-winedbg-watchpoint: windows pid unknown", file=sys.stderr)
        return 3
    report["windows_pid"] = winpid

    if args.addr:
        addr = int(args.addr, 0)
        report["addr_source"] = "explicit"
    else:
        raw = read_mem(pid, GAME_MAN_SINGLETON_VA, 8)
        if not raw or len(raw) != 8:
            report["error"] = f"could not read GameMan singleton at {GAME_MAN_SINGLETON_VA:#x}"
            (out / "watchpoint.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
            print(report["error"], file=sys.stderr)
            return 3
        game_man = struct.unpack("<Q", raw)[0]
        report["game_man"] = hex(game_man)
        if not game_man:
            report["error"] = "GameMan singleton is NULL -- the game has not built it yet"
            (out / "watchpoint.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
            print(report["error"], file=sys.stderr)
            return 4
        addr = game_man + int(args.field, 0)
        report["addr_source"] = f"GameMan+{args.field}"
    report["watch_addr"] = hex(addr)
    before = read_mem(pid, addr, 4)
    report["value_before"] = struct.unpack("<i", before)[0] if before and len(before) == 4 else None

    env_game = read_environ(pid)
    prefix = env_game.get("WINEPREFIX")
    if not prefix:
        report["error"] = "target has no WINEPREFIX in its environment"
        (out / "watchpoint.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(report["error"], file=sys.stderr)
        return 3
    steam_dir = Path(env_game.get("STEAM_COMPAT_CLIENT_INSTALL_PATH",
                                  str(Path.home() / ".local/share/Steam")))
    wine = proton_wine(steam_dir, args.proton)
    if not wine.exists():
        report["error"] = f"proton wine not found at {wine}"
        (out / "watchpoint.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(report["error"], file=sys.stderr)
        return 3
    report["wine"] = str(wine)
    report["wineprefix"] = prefix

    port = free_port()
    report["port"] = port
    env = dict(os.environ)
    for key in ("WINEPREFIX", "WINEDLLPATH", "WINEESYNC", "WINEFSYNC", "WINEDEBUG",
                "WINELOADERNOEXEC", "WINE_LARGE_ADDRESS_AWARE", "LD_LIBRARY_PATH"):
        if key in env_game:
            env[key] = env_game[key]
    env["WINEDEBUG"] = env.get("WINEDEBUG", "-all")
    # Do not inherit the game's WINEDLLOVERRIDES: winedbg needs none of it and the d3d
    # overrides only add failure modes to a debugger process.
    env.pop("WINEDLLOVERRIDES", None)

    proxy_cmd = [str(wine), "winedbg", "--gdb", "--no-start", "--port", str(port), str(winpid)]
    report["proxy_command"] = " ".join(shlex.quote(c) for c in proxy_cmd)
    proxy_log: list[str] = []
    deadline = time.monotonic() + args.deadline_seconds
    proxy = subprocess.Popen(proxy_cmd, env=env, stdout=subprocess.PIPE,
                             stderr=subprocess.STDOUT, text=True, bufsize=1)
    gdb_rc: int | None = None
    gdb_out = ""
    try:
        listening = wait_for_proxy_port(proxy, port, deadline)
        drain_proxy(proxy, proxy_log)
        report["proxy_listening"] = listening
        if not listening:
            report["error"] = "winedbg proxy never reached LISTEN on the requested port"
            report["proxy_log"] = proxy_log[-40:]
            (out / "watchpoint.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
            print(report["error"], file=sys.stderr)
            print("\n".join(proxy_log[-40:]), file=sys.stderr)
            return 5
        announced = port
        report["announced_port"] = announced
        script = gdb_script(addr, args.mode, args.max_hits, announced, args.control_hits)
        script_path = out / "watchpoint.gdb"
        script_path.write_text(script, encoding="utf-8")
        gdb = subprocess.Popen(["gdb", "-q", "-nx", "--nh", "-batch", "-x", str(script_path)],
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        try:
            remaining = max(1.0, deadline - time.monotonic())
            gdb_out, _ = gdb.communicate(timeout=remaining)
            gdb_rc = gdb.returncode
        except subprocess.TimeoutExpired:
            gdb.kill()
            gdb_out, _ = gdb.communicate()
            gdb_rc = None
            report["gdb_timed_out"] = True
    finally:
        if proxy.poll() is None:
            proxy.terminate()
            try:
                proxy.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proxy.kill()
        rest = proxy.stdout.read() if proxy.stdout else ""
        if rest:
            proxy_log.extend(rest.splitlines())

    (out / "watchpoint-gdb.txt").write_text(gdb_out, encoding="utf-8")
    (out / "watchpoint-proxy.txt").write_text("\n".join(proxy_log), encoding="utf-8")
    report["gdb_returncode"] = gdb_rc
    report["connected"] = CONNECTED in gdb_out
    report["armed"] = ARMED in gdb_out
    report["hardware"] = bool(re.search(r"Hardware (?:access \(read/write\) )?watchpoint \d+", gdb_out))
    report["software_watchpoint"] = bool(
        re.search(r"^\s*Watchpoint \d+", gdb_out, re.M)) and not report["hardware"]
    # `info watchpoints` types are the arming evidence: hw/acc watchpoint = a debug register,
    # bare "watchpoint" = gdb fell back to single-stepping the whole game.
    report["watchpoint_types"] = re.findall(
        r"^\s*\d+\s+((?:hw|acc|read)? ?watchpoint)\s", gdb_out, re.M)
    ctrl = re.findall(r"--WP-CTRL-HIT-- n=(\d+) rip=(0x[0-9a-fA-F]+)", gdb_out)
    report["control_hits"] = [{"n": int(n), "rip": r} for n, r in ctrl]
    report["control_hit_count"] = len(ctrl)
    report["control_proved_hardware_traps_deliver"] = len(ctrl) > 0
    proxy_text = "\n".join(proxy_log)
    crash = re.search(r"winedbg: Internal crash at ([0-9A-Fa-f]+)", proxy_text)
    report["winedbg_internal_crash"] = crash.group(1) if crash else None
    report["remote_closed_early"] = "Remote connection closed" in gdb_out
    hits = parse_hits(gdb_out)
    report["hits"] = hits
    report["hit_count"] = len(hits)
    report["one_to_zero"] = [h for h in hits if h.get("old") == 1 and h.get("new") == 0]
    still = find_pid("eldenring.exe")
    report["game_alive_after"] = bool(still)
    after = read_mem(still, addr, 4) if still else None
    report["value_after_session"] = struct.unpack("<i", after)[0] if after and len(after) == 4 else None
    (out / "watchpoint.json").write_text(json.dumps(report, indent=2), encoding="utf-8")

    print(f"watch {report['watch_addr']} ({report['addr_source']}) winpid={winpid} pid={pid}")
    print(f"connected={report['connected']} armed={report['armed']} "
          f"hardware={report['hardware']} software={report['software_watchpoint']} "
          f"types={report['watchpoint_types']}")
    print(f"CONTROL (awatch, same DR mechanism): hits={report['control_hit_count']} "
          f"-> traps deliver: {report['control_proved_hardware_traps_deliver']}")
    print(f"hits={report['hit_count']} one_to_zero={len(report['one_to_zero'])} "
          f"game_alive_after={report['game_alive_after']}")
    if report["winedbg_internal_crash"]:
        print(f"WINEDBG CRASHED at {report['winedbg_internal_crash']} -- the session died, so a "
              f"zero hit count here measures NOTHING about the field")
    for h in hits:
        print(f"  hit n={h.get('n')} rip={h.get('rip')} old={h.get('old')} new={h.get('new')}")
    print(f"evidence -> {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
