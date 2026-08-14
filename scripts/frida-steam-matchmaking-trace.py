#!/usr/bin/env python3
"""Trace how Seamless Co-op finds and joins a lobby, through the Steam API.

WHY THIS, AND WHY NOT A MEMORY DUMP
-----------------------------------
The functions that choose an invasion target -- ersc `0x18006a2d0`
(`seamless_session_manager.cpp:2014`) and `0x18006a1e0` (`yui_nexus_3.cpp`) -- are `e9` jumps
into `.themida`. A live dump of the running module (99.95% readable) shows those jump targets
are themselves chains of `e9` jumps. That is VIRTUALIZATION, not on-demand decryption: the
original x86 for those ~40 functions does not exist in memory in any form, so no dump can ever
recover them. Reading them would mean devirtualizing Themida.

The decision cannot stay inside the VM, though. To find and join a lobby it must call
`steam_api64.dll`, which is ordinary un-virtualized code in a separate module. Every filter it
applies and every lobby it picks crosses that boundary in the clear.

So: hook the matchmaking surface and record what Seamless asks Steam for. If the lobby search is
filtered (by a password-derived key or anything else), the filter calls show it with the actual
string. If it is not filtered, that absence is equally decisive -- and it is the difference
between "a unique password targets your friend" and "it does nothing", which has already been
asserted once in this repo without evidence and turned out to be wrong.

READ-ONLY. Arguments are logged, never altered. Hooking Steam's own exports does not touch
`ersc.dll` and cannot perturb its logic.

REACHING THE PROCESS
--------------------
The game runs under Wine/Proton, so a Linux-side `frida.attach()` sees nothing. `frida-gadget.dll`
is loaded into the game as an me3 `[[natives]]` entry and listens on 127.0.0.1:27042; we connect
to that as a REMOTE DEVICE. Launch with a gadget-bearing profile, e.g.
/home/banon/Elden/pr190-invasion-warp-seamless-frida.me3

RUN IT (uv provisions frida per-run; nothing is installed system-wide):
    uv run --with frida python3 /home/banon/projects/er-effects-rs/scripts/frida-steam-matchmaking-trace.py --list
    uv run --with frida python3 /home/banon/projects/er-effects-rs/scripts/frida-steam-matchmaking-trace.py

Artifacts land in /tmp (the session scratchpad by default), never in the repo.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import threading

GADGET = "127.0.0.1:27042"
DEFAULT_OUT = (
    "/tmp/claude-1000/-home-banon-projects-er-effects-rs/"
    "fdd5f467-bf36-402d-bbcd-6defe1f4d0b7/scratchpad/steam-matchmaking-trace.jsonl"
)

#: Substrings of the flat-API export names worth hooking.
#:
#: MEASURED 2026-08-04: hooking ONLY the lobby API and attaching after boot produced ZERO calls
#: across a complete invasion. Two reasons, and both are corrected here:
#:
#:  1. WRONG SURFACE. Seamless's matchmaking is its own, not the vanilla shape. Whatever selects
#:     a target has to move data between peers, so the networking APIs are added -- P2P sessions
#:     and SteamNetworkingMessages carry ERSC's own protocol, and rich presence / lobby member
#:     data are how a session advertises who is in it.
#:  2. WRONG TIME. A lobby joined during session setup is invisible to a tracer that attaches
#:     afterwards. Nothing here fixes that -- the tracer must be started at BOOT, before the
#:     session forms, or a per-session (rather than per-invasion) search is missed entirely.
#:
#: Kept broad on purpose: a call that never happens costs one idle hook, while a surface left
#: unhooked costs a whole run and reads as a false negative.
INTERESTING = [
    # Lobby search/join -- the vanilla-shaped surface. Retained so its ABSENCE stays on record.
    "AddRequestLobbyListStringFilter",
    "AddRequestLobbyListNumericalFilter",
    "AddRequestLobbyListNearValueFilter",
    "AddRequestLobbyListFilterSlotsAvailable",
    "AddRequestLobbyListResultCountFilter",
    "AddRequestLobbyListDistanceFilter",
    "RequestLobbyList",
    "GetLobbyByIndex",
    "JoinLobby",
    "CreateLobby",
    "SetLobbyData",
    "GetLobbyData",
    "SetLobbyMemberData",
    "GetNumLobbyMembers",
    "GetLobbyMemberByIndex",
    # ERSC's own transport. An invasion has to signal the host somehow, and this is where a
    # target id would cross the wire.
    "ISteamNetworkingMessages_SendMessageToUser",
    "ISteamNetworkingMessages_ReceiveMessagesOnChannel",
    "ISteamNetworkingMessages_AcceptSessionWithUser",
    "ISteamNetworkingSockets_ConnectP2P",
    "ISteamNetworkingSockets_SendMessageToConnection",
    "ISteamNetworkingSockets_AcceptConnection",
    "ISteamNetworking_SendP2PPacket",
    "ISteamNetworking_ReadP2PPacket",
    "ISteamNetworking_AcceptP2PSessionWithUser",
    # How a session advertises and discovers its members.
    "ISteamFriends_SetRichPresence",
    "ISteamFriends_GetFriendRichPresence",
    "ISteamFriends_RequestFriendRichPresence",
]

AGENT = r"""
const hooked = [];

function steamModule() {
  return Process.enumerateModules().find(m => /steam_api(64)?\.dll/i.test(m.name)) || null;
}

rpc.exports = {
  list: function () {
    const m = steamModule();
    if (m === null) return null;
    return {
      name: m.name,
      base: m.base.toString(),
      exports: m.enumerateExports().map(e => e.name),
    };
  },

  hook: function (names) {
    const m = steamModule();
    if (m === null) return { error: 'steam_api not loaded' };
    const exps = m.enumerateExports();
    for (const want of names) {
      for (const e of exps) {
        if (e.type !== 'function') continue;
        if (e.name.indexOf(want) === -1) continue;
        if (hooked.indexOf(e.name) !== -1) continue;
        try {
          const label = e.name;
          Interceptor.attach(e.address, {
            onEnter(args) {
              // Steam's flat API is (self, ...). Capture the leading args both as raw values
              // and, where the bytes look like printable text, as a string -- filter keys and
              // values are char*, and the string form is the whole point of the trace.
              const out = [];
              for (let i = 0; i < 5; i++) {
                let raw = '<?>', str = null;
                try { raw = args[i].toString(); } catch (err) { /* unreadable */ }
                try {
                  const s = args[i].readUtf8String(96);
                  if (s !== null && s.length > 0 && /^[\x20-\x7e]+$/.test(s)) str = s;
                } catch (err) { /* not a string */ }
                out.push({ raw: raw, str: str });
              }
              send({ type: 'call', fn: label, args: out });
            },
            onLeave(retval) {
              send({ type: 'ret', fn: label, ret: retval.toString() });
            },
          });
          hooked.push(e.name);
        } catch (err) {
          send({ type: 'hook-error', fn: e.name, error: err.message });
        }
      }
    }
    return { hooked: hooked };
  },
};
"""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gadget", default=GADGET)
    parser.add_argument("--out", default=DEFAULT_OUT, help="artifact path (must be outside the repo)")
    parser.add_argument("--list", action="store_true", help="print matchmaking exports and exit")
    args = parser.parse_args()

    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if os.path.abspath(args.out).startswith(repo):
        print(f"REFUSING: {args.out} is inside the repo; artifacts belong outside it.", file=sys.stderr)
        return 2

    try:
        sys.stdout.reconfigure(line_buffering=True)
    except (AttributeError, ValueError):
        pass

    try:
        import frida
    except ImportError:
        print(
            "ERROR: frida is not importable. It is deliberately not installed system-wide; uv "
            "provisions it per-run:\n  uv run --with frida python3 "
            "/home/banon/projects/er-effects-rs/scripts/frida-steam-matchmaking-trace.py",
            file=sys.stderr,
        )
        return 7

    try:
        device = frida.get_device_manager().add_remote_device(args.gadget)
        session = device.attach("Gadget")
    except Exception as exc:
        print(
            f"ERROR: could not reach frida-gadget at {args.gadget}: {exc}\n"
            "Is the game running with a profile that includes frida-gadget.dll?",
            file=sys.stderr,
        )
        return 3

    script = session.create_script(AGENT)
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    handle = open(args.out, "w", encoding="utf-8")
    count = 0

    def on_message(message, _data):
        nonlocal count
        if message.get("type") != "send":
            print(f"[agent] {message}", file=sys.stderr)
            return
        payload = message.get("payload") or {}
        handle.write(json.dumps(payload) + "\n")
        handle.flush()
        if payload.get("type") == "call":
            count += 1
            shown = [
                f'"{a["str"]}"' if a.get("str") else a["raw"] for a in payload["args"]
            ]
            print(f"{payload['fn']}({', '.join(shown)})")
        elif payload.get("type") == "ret":
            print(f"    -> {payload['ret']}")
        elif payload.get("type") == "hook-error":
            print(f"[hook-error] {payload['fn']}: {payload['error']}", file=sys.stderr)

    script.on("message", on_message)
    script.load()

    if args.list:
        info = script.exports_sync.list()
        if info is None:
            print("steam_api64.dll is not loaded", file=sys.stderr)
            return 4
        print(f"{info['name']} base={info['base']} exports={len(info['exports'])}")
        for name in sorted(n for n in info["exports"] if "Lobby" in n or "Matchmaking" in n):
            print(f"  {name}")
        return 0

    result = script.exports_sync.hook(INTERESTING)
    print(result)
    if not result.get("hooked"):
        print(
            "NOTHING WAS HOOKED. That is a result, not a dud: either steam_api64 is not loaded "
            "or its export names differ from the flat-API shape assumed here. Run --list.",
            file=sys.stderr,
        )
    print(f"tracing -> {args.out}")
    print("Now invade. Every lobby filter, search and join Seamless asks Steam for is recorded.")

    done = threading.Event()
    script.on("destroyed", done.set)
    if sys.stdin.isatty():
        try:
            sys.stdin.read()
        except KeyboardInterrupt:
            pass
    else:
        # Detached: run until the game exits. An invasion takes as long as it takes.
        print("detached: no tty, tracing until the game exits")
        done.wait()

    handle.close()
    print(f"{count} matchmaking call(s) -> {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
