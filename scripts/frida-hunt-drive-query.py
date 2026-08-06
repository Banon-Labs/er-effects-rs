#!/usr/bin/env python3
"""Drive ONE lobby query through the game's own Steam vtable, to see whether hunt narrows it.

WHY THIS EXISTS
---------------
`oracle_invasion_warp_hunt_hooked` says the detour is installed on
`ISteamMatchmaking::RequestLobbyList` (vtable slot 4, inside `steamclient64.dll`). It does NOT say
the detour ever ran, and it cannot: the hook only fires when somebody searches. Waiting for the
player to start an invasion means the decisive fact -- does our filter actually reach the wire --
is hostage to a menu action, and a run can end having proven only that a hook was installed.

So this calls slot 4 itself. That is the SAME slot ersc calls (measured 2026-08-06: ersc's own
query returned 13 lobbies through this interface), so the call exercises the identical path: our
detour runs, `hunt_target` picks a location, `AddRequestLobbyListStringFilter` goes on, and the
original body sends it. What it does NOT prove is that ersc triggers a query -- that was already
measured and is not in question here.

WHAT IT TOUCHES
---------------
One outgoing lobby query, which is a read: it asks Steam for a list. Nothing is published, no
session is started, no game state is written, no other player is affected. Seamless issues these
continuously while searching, so one more is ordinary traffic.

The one real hazard, and why the timing is deliberate: `RequestLobbyList` CONSUMES whatever filters
have been accumulated on the interface. Calling it while ersc has staged filters but not yet sent
would eat them, and ersc's own next query would go out wider than it intended. ersc stages and
sends in one burst, so the window is small -- but it is not zero, which is why this is a one-shot
diagnostic and not something to leave running.

    uv run --with frida python3 scripts/frida-hunt-drive-query.py
    uv run --with frida python3 scripts/frida-hunt-drive-query.py --own-lobby 0x1860000164a8cd9
    python3 scripts/frida-hunt-drive-query.py --selftest
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time

GADGET = "127.0.0.1:27042"
#: Steam's own accessor for the versioned matchmaking interface -- the same symbol the DLL uses,
#: so both resolve the identical process-wide singleton.
ACCESSOR = "SteamAPI_SteamMatchmaking_v009"
SLOT_REQUEST_LOBBY_LIST = 4
SLOT_GET_LOBBY_BY_INDEX = 12


def _default_out() -> str:
    """Repo-relative by default; `ER_PROBE_OUT_DIR` overrides.

    Not an absolute session path: a capture path containing one machine's uid and one agent
    session id is correct exactly once and silently wrong every time after.
    """
    override = os.environ.get("ER_PROBE_OUT_DIR")
    if override:
        return os.path.join(override, "hunt-drive-query.json")
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    return os.path.join(repo, "target", "steam-probe", "hunt-drive-query.json")


AGENT = r"""
const SLOT_REQUEST_LOBBY_LIST = 4;
const SLOT_GET_LOBBY_BY_INDEX = 12;
// Bounded: an unbounded walk in a live process is its own hazard, and no Seamless query has ever
// been observed returning anything close to this.
const MAX_INDEX = 200;

rpc.exports = {
  drive: function (accessorName, ownLobby, pollMs, pollTicks) {
    // Resolved by walking steam_api64's own export table rather than through
    // `Module.findExportByName`, which Frida 17 removed -- the sibling tracer in this repo already
    // uses this idiom and works against the same process.
    const mod = Process.enumerateModules().find(m => /steam_api(64)?\.dll/i.test(m.name)) || null;
    if (mod === null) return { ok: false, why: 'steam_api64.dll not loaded' };
    let acc = null;
    for (const e of mod.enumerateExports()) {
      if (e.type === 'function' && e.name === accessorName) { acc = e.address; break; }
    }
    if (acc === null) return { ok: false, why: 'accessor ' + accessorName + ' not exported' };
    let iface;
    try {
      iface = new NativeFunction(acc, 'pointer', [])();
    } catch (e) { return { ok: false, why: 'accessor threw: ' + e }; }
    if (iface.isNull()) return { ok: false, why: 'accessor returned null' };

    let vt, req, byIndex;
    try {
      vt = iface.readPointer();
      req = new NativeFunction(vt.add(SLOT_REQUEST_LOBBY_LIST * Process.pointerSize).readPointer(),
                               'uint64', ['pointer']);
      byIndex = new NativeFunction(vt.add(SLOT_GET_LOBBY_BY_INDEX * Process.pointerSize).readPointer(),
                                   'pointer', ['pointer', 'pointer', 'int']);
    } catch (e) { return { ok: false, why: 'vtable unreadable: ' + e }; }

    let call;
    try {
      call = req(iface).toString();
    } catch (e) { return { ok: false, why: 'RequestLobbyList threw: ' + e }; }

    // Results arrive asynchronously through a callback inside ersc's Themida region, so there is
    // nothing to hook for "ready". Poll and keep the high-water set.
    const out = Memory.alloc(8);
    let best = [], deadline = Date.now() + pollMs * pollTicks;
    while (Date.now() < deadline) {
      const ids = [];
      for (let i = 0; i < MAX_INDEX; i++) {
        try {
          out.writeU64(0);
          byIndex(iface, out, i);
          const id = out.readU64();
          if (id.compare(0) === 0) break;
          ids.push(id.toString(16));
        } catch (e) { break; }
      }
      if (ids.length > best.length) best = ids;
      Thread.sleep(pollMs / 1000);
    }

    const own = ownLobby ? ownLobby.toLowerCase().replace(/^0x/, '') : null;
    return {
      ok: true,
      iface: iface.toString(),
      vtable: vt.toString(),
      api_call: call,
      results: best.length,
      lobbies: best,
      own_lobby: own,
      own_lobby_returned: own === null ? null : best.indexOf(own) >= 0,
    };
  },
};
"""


def verdict(driven: dict, before: dict, after: dict) -> dict:
    """What the driven query establishes, from the DLL's own oracle counters either side of it.

    The counters are the evidence, not this script's return value: the query could succeed while
    the detour declined to filter, and only `hunt_filters` moving separates those.
    """
    if not driven.get("ok"):
        return {"verdict": "no-query-sent", "why": driven.get("why", "unknown")}

    hooked = after.get("oracle_invasion_warp_hunt_hooked")
    delta = (after.get("oracle_invasion_warp_hunt_filters") or 0) - (
        before.get("oracle_invasion_warp_hunt_filters") or 0
    )
    out = {
        "filters_added_by_this_query": delta,
        "hooked": hooked,
        "results": driven.get("results"),
        "own_lobby_returned": driven.get("own_lobby_returned"),
    }
    if hooked is not True:
        out["verdict"] = "hook-absent"
        out["why"] = (
            "the query went out, but the detour is not installed -- nothing was narrowed and "
            "hunt is inert regardless of the config"
        )
    elif delta <= 0:
        out["verdict"] = "hooked-but-declined"
        out["why"] = (
            "the detour ran and chose NOT to filter. That is hunt off, an unreadable block, or "
            "several marked locations a single equality filter cannot express -- the DLL log says "
            "which. It is not evidence against the transport"
        )
    elif driven.get("results"):
        out["verdict"] = "FILTERED-AND-MATCHED"
        out["why"] = (
            f"the query carried our location filter and still returned {driven['results']} "
            "lobby(ies) -- a host publishing our key at that location was found"
        )
    else:
        out["verdict"] = "FILTERED-NO-HOST"
        out["why"] = (
            "the filter reached the wire and the query returned nobody. Expected while no other "
            "player runs this DLL: it proves the transport, NOT a match"
        )
    return out


def _selftest() -> int:
    fails = 0

    def check(ok: bool, label: str) -> None:
        nonlocal fails
        print(f"  {'ok  ' if ok else 'FAIL'} {label}")
        if not ok:
            fails += 1

    def counters(hooked, filters):
        return {
            "oracle_invasion_warp_hunt_hooked": hooked,
            "oracle_invasion_warp_hunt_filters": filters,
        }

    check(
        verdict({"ok": False, "why": "accessor not found"}, {}, {})["verdict"] == "no-query-sent",
        "a query that never went out is not a finding about filtering",
    )

    # THE TRAP: a query that returns nothing while the detour DECLINED to filter looks exactly
    # like a working filter finding nobody. Only the counter delta separates them, and calling
    # the first case proof would be inventing a result.
    v = verdict({"ok": True, "results": 0}, counters(True, 4), counters(True, 4))
    check(v["verdict"] == "hooked-but-declined",
          "no filter added means declined, never 'the filter found nobody'")

    v = verdict({"ok": True, "results": 0}, counters(True, 0), counters(True, 1))
    check(v["verdict"] == "FILTERED-NO-HOST" and "NOT a match" in v["why"],
          "an empty filtered query proves the transport and says so, without claiming a match")

    v = verdict({"ok": True, "results": 2}, counters(True, 0), counters(True, 1))
    check(v["verdict"] == "FILTERED-AND-MATCHED",
          "a filtered query that returns hosts is the end-to-end result")

    v = verdict({"ok": True, "results": 9}, counters(False, 0), counters(False, 0))
    check(v["verdict"] == "hook-absent",
          "results without a hook are unfiltered results, not a hunt success")

    if fails:
        print(f"selftest FAILED ({fails})")
        return 1
    print("selftest ok")
    return 0


def read_oracles(path: str) -> dict:
    try:
        with open(path, encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, ValueError):
        return {}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--gadget", default=GADGET)
    ap.add_argument("--own-lobby", help="this client's lobby id, to test whether a query returns it")
    ap.add_argument(
        "--telemetry",
        default="/home/banon/.local/share/Steam/steamapps/common/ELDEN RING/Game/"
        "er-invasion-warp-telemetry.json",
        help="the DLL's oracle document, sampled either side of the query",
    )
    ap.add_argument("--poll-ms", type=int, default=250)
    ap.add_argument("--poll-ticks", type=int, default=24)
    ap.add_argument("--out", default=_default_out())
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        return _selftest()

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

    before = read_oracles(args.telemetry)
    script = session.create_script(AGENT)
    script.load()
    print(f"driving one RequestLobbyList through {ACCESSOR} ...")
    driven = script.exports_sync.drive(
        ACCESSOR, args.own_lobby, args.poll_ms, args.poll_ticks
    )
    # The DLL republishes its oracle document on its own tick, so give it one before sampling.
    time.sleep(2.0)
    after = read_oracles(args.telemetry)

    result = {"driven": driven, "verdict": verdict(driven, before, after)}
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    with open(args.out, "w", encoding="utf-8") as handle:
        json.dump(result, handle, indent=2)
    print(json.dumps(result, indent=2))
    print(f"\nwrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
