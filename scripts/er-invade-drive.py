#!/usr/bin/env python3
"""Hand er_invasion_warp.dll Seamless's menu object, then start an invasion, agent-driven.

# Why this exists

AGENTS.md: the agent drives every required input, and asking the user to press a button is an
instruction-following failure. Starting an invasion needs exactly one value -- the option-menu
object -- because `ersc+0x25850` reads `rcx` and nothing else. `scripts/frida/ersc-handoff.js`
finds that object by walking ersc.dll's writable data, so no press is needed to obtain it.

It also hands the pointer to `er_invasion_warp.dll` through
`er_invasion_warp_adopt_menu_object`, which is how the DLL resolves the session without detouring
`ersc.dll` itself -- its own detour there faults the game at 29.3s (three runs, STATUS_ILLEGAL_-
instruction at eldenring.exe+0x10043), while a Frida hook on the same address does not.

    python3 scripts/er-frida-up.py
    uv run --with frida python3 scripts/er-invade-drive.py            # hand over + invade
    uv run --with frida python3 scripts/er-invade-drive.py --find     # hand over only
    uv run --with frida python3 scripts/er-invade-drive.py --cancel
    uv run --with frida python3 scripts/er-invade-drive.py --selftest
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import pathlib
import re
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import er_run_lib

_spec = importlib.util.spec_from_file_location(
    "er_frida_watch", pathlib.Path(__file__).resolve().parent / "er-frida-watch.py"
)
_watch = importlib.util.module_from_spec(_spec)
assert _spec.loader is not None
_spec.loader.exec_module(_watch)

AGENT = pathlib.Path(__file__).resolve().parent / "frida" / "ersc-handoff.js"
RUNS = pathlib.Path.home() / ".cache" / "er-me3-runs"
# The DLL says where the pair is, in its own log, the moment its scan succeeds. Reading that line
# is better than reimplementing the scan a third time -- and on 2026-09-08 the Frida-side scan
# found nothing across all of ersc.dll's writable data while this line was already sitting in the
# log with both addresses in it.
OWNER_LINE = re.compile(
    r"session resolved WITHOUT hooking Seamless -- found at 0x([0-9a-f]+).*?owner 0x([0-9a-f]+)"
)


def owner_from_dll_log() -> tuple:
    """(session, owner) as hex strings from the newest run's log, or (None, None)."""
    runs = sorted(RUNS.glob("br-*"), key=lambda d: d.stat().st_mtime, reverse=True)
    for run in runs[:3]:
        log = run / "er-invasion-warp.log"
        if not log.exists():
            continue
        match = None
        for line in log.read_text(encoding="utf-8", errors="replace").splitlines():
            found = OWNER_LINE.search(line)
            if found:
                match = found
        if match:
            return ("0x" + match.group(1), "0x" + match.group(2))
    return (None, None)
LOG = pathlib.Path.home() / ".cache" / "er-frida" / "hits.jsonl"
# After driving an invade, how long to keep streaming so the DLL's judgement, its cancel and its
# banner all land in the same transcript as the drive that caused them.
FOLLOW_SECONDS = 90.0


def selftest() -> int:
    # The agent is the part that can silently stop matching the DLL. Assert the two names it calls
    # across the boundary are exactly the ones the DLL exports, read out of the built artifact.
    source = AGENT.read_text(encoding="utf-8")
    assert "er_invasion_warp_adopt_menu_object" in source
    assert "er_invasion_warp.dll" in source
    # The log line this driver depends on must still be the one the DLL writes.
    dll_source = pathlib.Path(
        "crates/er-invasion-warp/src/local_invasion_filter.rs"
    )
    if dll_source.exists():
        assert OWNER_LINE.search(
            "session resolved WITHOUT hooking Seamless -- found at 0xdeadbeef via a pointer in "
            "ersc's own writable data at 0x1802f2680, owner 0xcafef00d"
        ).groups() == ("deadbeef", "cafef00d"), "the owner parser must read both addresses"
        assert "session resolved WITHOUT hooking Seamless" in dll_source.read_text(
            encoding="utf-8"
        ), "the DLL no longer writes the line this driver parses"
    dll = pathlib.Path("target/x86_64-pc-windows-msvc/release/er_invasion_warp.dll")
    if dll.exists():
        blob = dll.read_bytes()
        assert b"er_invasion_warp_adopt_menu_object" in blob, (
            "the DLL does not export the name the agent calls -- rebuild er-invasion-warp"
        )
        print("selftest ok: agent and built DLL agree on the export name")
    else:
        print("selftest ok: agent names the export (DLL not built, so it was not cross-checked)")
    own_source = pathlib.Path(__file__).read_text(encoding="utf-8")
    assert "er_run_lib.wait_for_exit(game, args.follow)" in own_source, (
        "the follow window must block on the game's pidfd, so a game that dies mid-follow ends "
        "the drive instead of streaming silence at a dead process"
    )
    # Built from pieces so the assertion does not match its own source text.
    assert ("ti" + "me.sl" + "eep(") not in own_source, "the driver must not sleep"
    print("selftest ok: the follow window is an event wait, not a poll loop")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--find", action="store_true", help="hand the pointer over, drive nothing")
    parser.add_argument("--cancel", action="store_true", help="drive cancel instead of invade")
    parser.add_argument("--follow", type=float, default=FOLLOW_SECONDS)
    parser.add_argument("--osm", default=None, help="menu object to drive (default: the DLL's)")
    parser.add_argument(
        "--direct",
        action="store_true",
        help="call ersc's invade from this thread instead of asking the DLL to do it on the game "
        "thread. Diagnostic only: measured to park on the session mutex.",
    )
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    dev = _watch.device()
    try:
        pid = _watch.find_game_bounded(dev)
    except TimeoutError:
        print("the frida server did not answer -- restart it with --force", file=sys.stderr)
        return 2
    if pid is None:
        print("no eldenring.exe in the prefix", file=sys.stderr)
        return 1
    frida_session = dev.attach(pid)
    LOG.parent.mkdir(parents=True, exist_ok=True)
    log = open(LOG, "a", buffering=1, encoding="utf-8")

    def on_message(message, _data):
        log.write(json.dumps({"at": time.time(), "message": message}) + "\n")
        print(f"AGENT {message.get('payload', message)}", flush=True)

    script = frida_session.create_script(AGENT.read_text(encoding="utf-8"))
    script.on("message", on_message)
    script.load()

    osm = args.osm
    found_session = None
    if osm is None:
        found_session, osm = owner_from_dll_log()
        print(f"from the DLL's log: session={found_session} owner={osm}", flush=True)
    if (osm is None or int(osm, 16) == 0) and found_session is not None:
        # `owner 0x0` means the DLL found the session but nothing that owns it, and both ersc
        # actions take the owner as `rcx` and read the session out of `[rcx+0x58]` -- so a bare
        # session cannot drive anything, and passing 0 is a null read inside ersc.dll.
        #
        # The owner is findable without waiting for the player: scan writable memory for a qword
        # holding the session pointer, and the address that holds it is `owner + 0x58`. The DLL's
        # own validation decides which candidate is real.
        print(f"owner is 0 -- scanning for whatever holds {found_session}", flush=True)
        osm = script.exports_sync.adopt_owner_of(found_session)
        print(f"adopted owner: {osm}", flush=True)
    if osm is None or int(osm, 16) == 0:
        print("no usable menu object: nothing in memory holds the session the DLL found",
              file=sys.stderr)
        return 3
    print(f"state before: {script.exports_sync.state_at(osm)}", flush=True)
    if not args.find:
        if args.cancel:
            result = script.exports_sync.cancel_at(osm)
            print(f"cancel at {osm}: {result}", flush=True)
        elif args.direct:
            result = script.exports_sync.invade_at(osm)
            print(f"invade (direct) at {osm}: {result}", flush=True)
        else:
            result = script.exports_sync.request_invade()
            print(f"invade requested on the game thread: armed={result}", flush=True)
    # Stay attached so the DLL's judgement, its cancel and its banner all land in the same
    # transcript as the drive that caused them. Messages arrive on Frida's own threads while this
    # one is parked, so the follow window is spent blocking on the game's pidfd rather than waking
    # twice a second to do nothing: a game that dies mid-follow ends this at once, with the
    # transcript flushed, instead of streaming silence for another minute at a dead process.
    game = _watch.linux_game_pid()
    if game is None:
        print("cannot resolve the game's linux pid; following blind", file=sys.stderr)
        game = os.getpid()  # never exits, so this is a bounded block on a real descriptor
    if er_run_lib.wait_for_exit(game, args.follow):
        print("the game exited during the follow window", flush=True)
    frida_session.detach()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
