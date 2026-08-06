#!/usr/bin/env python3
"""Read the keys on one Steam lobby, to tell a HOST advertisement from an invader's search lobby.

WHY
---
`lobby_publish` gates publishing on "a non-zero lobby id exists at session+0x178 AND it carries
`lobby_type = yknx3_seamless_master_lobby`". Measured 2026-08-06, that gate is not host-specific:
an INVADER's search session populates the same field at session state 0x15, and the publish fired
during a search. An invader advertising a location is meaningless at best -- nobody can invade an
invader -- and at worst it pollutes another hunter's filtered results with lobbies that are not
joinable worlds.

So the question is whether Seamless itself publishes something that distinguishes the two. A host
run captured `lobby_breakin_lobby_ykssr_199_6 = "true"` and
`matchmaking_breakin_lobby_ykssr_199_6 = "4_3"`. If a search lobby lacks those, one of them is the
discriminator the publish gate should use.

READ ONLY. `GetLobbyData` (vtable slot 19) changes nothing for any player; this writes nothing and
starts no session.

    uv run --with frida python3 scripts/frida-lobby-read-keys.py --lobby 0x18600001692080e
    python3 scripts/frida-lobby-read-keys.py --selftest
"""

from __future__ import annotations

import argparse
import json
import sys

GADGET = "127.0.0.1:27042"
ACCESSOR = "SteamAPI_SteamMatchmaking_v009"

#: Every key a Seamless host was observed publishing, plus ours. Asking for a key that is absent
#: returns an empty string, which is exactly the signal being looked for.
KNOWN_KEYS = (
    "lobby_type",
    "lobby_key",
    "lobby_preferences",
    "ykssr_dlc",
    "lobby_breakin_lobby_ykssr_199_6",
    "matchmaking_breakin_lobby_ykssr_199_6",
    "er_invasion_warp_map",
)

AGENT = r"""
const SLOT_GET_LOBBY_DATA = 19;
// `ISteamMatchmaking::GetLobbyOwner` -- vtable slot 35. Read only, and the whole ballgame for the
// publish path: SetLobbyData persists only for the lobby OWNER. A non-owner write can succeed into
// the local copy and then be replaced by the server's version, which is exactly the observed
// shape -- call returns true, our counter says published, the key reads back empty.
const SLOT_GET_LOBBY_OWNER = 35;

rpc.exports = {
  read: function (accessorName, lobbyHex, keys) {
    const mod = Process.enumerateModules().find(m => /steam_api(64)?\.dll/i.test(m.name)) || null;
    if (mod === null) return { ok: false, why: 'steam_api64.dll not loaded' };
    let acc = null;
    for (const e of mod.enumerateExports()) {
      if (e.type === 'function' && e.name === accessorName) { acc = e.address; break; }
    }
    if (acc === null) return { ok: false, why: 'accessor not exported' };

    let iface, vt;
    try {
      iface = new NativeFunction(acc, 'pointer', [])();
      vt = iface.readPointer();
    } catch (e) { return { ok: false, why: 'interface unreadable: ' + e }; }

    const lobby = uint64(lobbyHex);
    const get = new NativeFunction(vt.add(SLOT_GET_LOBBY_DATA * Process.pointerSize).readPointer(),
                                   'pointer', ['pointer', 'uint64', 'pointer']);
    const out = {};
    for (const k of keys) {
      try {
        const kp = Memory.allocUtf8String(k);
        const r = get(iface, lobby, kp);
        // readCString, never readUtf8String(N): the length form THROWS when fewer than N bytes are
        // mapped, which silently turned every readable key into null in an earlier capture.
        out[k] = r.isNull() ? null : r.readCString();
      } catch (e) { out[k] = '<threw: ' + e + '>'; }
    }
    // Who owns this lobby, and are we them? `GetLobbyOwner` returns a CSteamID by value; the
    // local user's id comes from SteamUser's GetSteamID (slot 2 of ISteamUser).
    // An 8-byte CSteamID return goes through a HIDDEN RETURN POINTER in this build -- proven by
    // GetLobbyByIndex, which only works when called as (this, sret, index). Calling it as a plain
    // uint64 return put a sentinel (0xbeef) in the argument slot and faulted.
    let owner = null, me = null;
    try {
      const getOwner = new NativeFunction(
        vt.add(SLOT_GET_LOBBY_OWNER * Process.pointerSize).readPointer(),
        'pointer', ['pointer', 'pointer', 'uint64']);
      const out64 = Memory.alloc(8);
      out64.writeU64(0);
      getOwner(iface, out64, lobby);
      owner = '0x' + out64.readU64().toString(16);
    } catch (e) { owner = '<threw: ' + e + '>'; }
    // ISteamUser's accessor is versioned and the suffix varies by SDK; find whichever is exported
    // rather than guessing one and reporting a guess as a measurement.
    try {
      let userAcc = null, userName = null;
      for (const e of mod.enumerateExports()) {
        if (e.type === 'function' && /^SteamAPI_SteamUser_v\d+$/.test(e.name)) {
          userAcc = e.address; userName = e.name; break;
        }
      }
      if (userAcc === null) { me = '<no SteamAPI_SteamUser_vNNN export>'; }
      else {
        const user = new NativeFunction(userAcc, 'pointer', [])();
        const uvt = user.readPointer();
        // ISteamUser::GetSteamID -- slot 2, same sret convention.
        const getSteamId = new NativeFunction(uvt.add(2 * Process.pointerSize).readPointer(),
                                              'pointer', ['pointer', 'pointer']);
        const mine = Memory.alloc(8);
        mine.writeU64(0);
        getSteamId(user, mine);
        me = '0x' + mine.readU64().toString(16) + ' (via ' + userName + ')';
      }
    } catch (e) { me = '<threw: ' + e + '>'; }

    return { ok: true, iface: iface.toString(), lobby: lobbyHex, values: out,
             owner: owner, local_user: me };
  },
};
"""


def discriminator(values: dict) -> dict:
    """Which observed key, if any, separates a host advertisement from a search lobby.

    A key counts only when it is PRESENT and non-empty. Steam returns an empty string for a key a
    lobby does not carry, so "" and absent are the same answer and neither is evidence of a host.
    """
    present = {k: v for k, v in values.items() if v not in (None, "")}
    breakin = present.get("lobby_breakin_lobby_ykssr_199_6")
    matchmaking = present.get("matchmaking_breakin_lobby_ykssr_199_6")
    is_master = present.get("lobby_type") == "yknx3_seamless_master_lobby"
    return {
        "present_keys": sorted(present),
        "carries_advertisement_marker": is_master,
        "carries_breakin_flag": breakin is not None,
        "breakin_value": breakin,
        "matchmaking_value": matchmaking,
        "verdict": (
            "host-advertisement"
            if is_master and breakin is not None
            else "master-lobby-without-breakin"
            if is_master
            else "not-an-advertisement-lobby"
        ),
    }


def _selftest() -> int:
    fails = 0

    def check(ok: bool, label: str) -> None:
        nonlocal fails
        print(f"  {'ok  ' if ok else 'FAIL'} {label}")
        if not ok:
            fails += 1

    host = {
        "lobby_type": "yknx3_seamless_master_lobby",
        "lobby_breakin_lobby_ykssr_199_6": "true",
        "er_invasion_warp_map": "m28_00_00_00",
    }
    check(discriminator(host)["verdict"] == "host-advertisement", "a host advertisement is named")

    # THE TRAP: Steam returns "" for a key the lobby does not carry. Treating an empty string as
    # presence would make every lobby look like a host and the discriminator useless.
    search = {
        "lobby_type": "yknx3_seamless_master_lobby",
        "lobby_breakin_lobby_ykssr_199_6": "",
        "er_invasion_warp_map": "m28_00_00_00",
    }
    check(
        discriminator(search)["verdict"] == "master-lobby-without-breakin",
        "an empty break-in value is absence, not a host",
    )
    check(
        not discriminator(search)["carries_breakin_flag"],
        "empty string must not count as carrying the flag",
    )
    check(
        discriminator({"lobby_type": None})["verdict"] == "not-an-advertisement-lobby",
        "a lobby without the master marker is neither",
    )

    if fails:
        print(f"selftest FAILED ({fails})")
        return 1
    print("selftest ok")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--lobby", help="lobby CSteamID, hex (e.g. 0x18600001692080e)")
    ap.add_argument("--gadget", default=GADGET)
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        return _selftest()
    if not args.lobby:
        ap.error("--lobby is required")

    try:
        import frida
    except ImportError:
        print("ERROR: run under `uv run --with frida python3`", file=sys.stderr)
        return 7
    try:
        device = frida.get_device_manager().add_remote_device(args.gadget)
        session = device.attach("Gadget")
    except Exception as exc:
        print(f"ERROR: gadget unreachable: {exc}", file=sys.stderr)
        return 3

    script = session.create_script(AGENT)
    script.load()
    result = script.exports_sync.read(ACCESSOR, args.lobby, list(KNOWN_KEYS))
    if result.get("ok"):
        result["discriminator"] = discriminator(result["values"])
    print(json.dumps(result, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
