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
#: Steam's own accessor for the versioned matchmaking interface. NOT a process-wide singleton, as
#: this once claimed: on 2026-08-06 it returned four different pointers in one session
#: (0x4604fbf0, 0x45d027a0, 0x45f8afe0, 0x460367e0). The vtable behind them is shared, which is why
#: slot calls still work, but nothing here may assume the object is stable across calls.
ACCESSOR = "SteamAPI_SteamMatchmaking_v009"
SLOT_REQUEST_LOBBY_LIST = 4
SLOT_GET_LOBBY_BY_INDEX = 12
SLOT_GET_LOBBY_DATA = 19

#: The key our filter narrows on. A result set attributable to OUR filtered query must carry it on
#: every member; see `attribution` below for why that check is not optional.
LOBBY_MAP_KEY = "er_invasion_warp_map"


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
const SLOT_GET_LOBBY_DATA = 19;
// Bounded: an unbounded walk in a live process is its own hazard, and no Seamless query has ever
// been observed returning anything close to this.
const MAX_INDEX = 200;

rpc.exports = {
  drive: function (accessorName, ownLobby, pollMs, pollTicks, mapKey) {
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

    let vt, req, byIndex, getData;
    try {
      vt = iface.readPointer();
      req = new NativeFunction(vt.add(SLOT_REQUEST_LOBBY_LIST * Process.pointerSize).readPointer(),
                               'uint64', ['pointer']);
      byIndex = new NativeFunction(vt.add(SLOT_GET_LOBBY_BY_INDEX * Process.pointerSize).readPointer(),
                                   'pointer', ['pointer', 'pointer', 'int']);
      getData = new NativeFunction(vt.add(SLOT_GET_LOBBY_DATA * Process.pointerSize).readPointer(),
                                   'pointer', ['pointer', 'uint64', 'pointer']);
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

    // ATTRIBUTION. The poll reads the interface's CURRENT result set, which is a shared slot: ersc
    // issues its own queries continuously while searching, and whichever completed last is what
    // sits there. So the returned lobbies are NOT necessarily the answer to the query driven here.
    // Read our key off each one; a set produced by a filter on that key cannot contain a lobby
    // that lacks it. Without this the verdict was inferred from timing alone, and it produced two
    // false FILTERED-AND-MATCHED readings on 2026-08-06, both since withdrawn.
    const carrying = [];
    if (mapKey) {
      const kp = Memory.allocUtf8String(mapKey);
      for (const hex of best) {
        try {
          const r = getData(iface, uint64('0x' + hex), kp);
          // readCString, not readUtf8String(N): the length form throws when fewer than N bytes are
          // mapped, which silently reports every readable key as absent.
          const v = r.isNull() ? null : r.readCString();
          if (v !== null && v !== '') carrying.push({ lobby: hex, value: v });
        } catch (e) { /* unreadable lobby, counted as not carrying */ }
      }
    }

    const own = ownLobby ? ownLobby.toLowerCase().replace(/^0x/, '') : null;
    return {
      ok: true,
      iface: iface.toString(),
      vtable: vt.toString(),
      api_call: call,
      results: best.length,
      lobbies: best,
      map_key: mapKey,
      carrying_map_key: carrying,
      own_lobby: own,
      own_lobby_returned: own === null ? null : best.indexOf(own) >= 0,
    };
  },
};
"""


def attribution(driven: dict) -> dict:
    """Can the polled result set be attributed to the query this script drove?

    It cannot be assumed. The result set lives in a slot on the interface, ersc issues its own
    queries continuously while searching, and the poll reads whatever completed most recently.

    The test is a property of the filter, not of timing: a set produced by an equality filter on
    `er_invasion_warp_map` cannot contain a lobby that does not carry that key. So if any returned
    lobby lacks it, the set is somebody else's query and says nothing about ours.

    Measured 2026-08-06: six lobbies came back, NONE carried the key, and the verdict still read
    FILTERED-AND-MATCHED twice. Both were withdrawn. This is the check that was missing.
    """
    results = driven.get("results") or 0
    carrying = driven.get("carrying_map_key") or []
    if not results:
        return {"attributable": None, "carrying": 0,
                "note": "no lobbies returned, so there is nothing to attribute"}
    if not driven.get("map_key"):
        return {"attributable": None, "carrying": 0,
                "note": "our key was not read back, so attribution was not tested"}
    if len(carrying) == results:
        return {"attributable": True, "carrying": len(carrying),
                "note": f"all {results} returned lobbies carry {driven['map_key']}, consistent "
                        "with a result set produced by our filter"}
    return {
        "attributable": False,
        "carrying": len(carrying),
        "note": f"{results - len(carrying)} of {results} returned lobbies do NOT carry "
                f"{driven['map_key']}; a filter on that key could not have produced this set, so "
                "it belongs to another query -- most likely one of ersc's own",
    }


def verdict(driven: dict, before: dict, after: dict) -> dict:
    """What the driven query establishes, from the DLL's own oracle counters either side of it.

    The counters are the evidence, not this script's return value: the query could succeed while
    the detour declined to filter, and only `hunt_filters` moving separates those. And even a
    genuine filter does not license reading the polled lobbies as its answer -- see `attribution`.
    """
    if not driven.get("ok"):
        return {"verdict": "no-query-sent", "why": driven.get("why", "unknown")}

    hooked = after.get("oracle_invasion_warp_hunt_hooked")
    delta = (after.get("oracle_invasion_warp_hunt_filters") or 0) - (
        before.get("oracle_invasion_warp_hunt_filters") or 0
    )
    attrib = attribution(driven)
    out = {
        "filters_added_by_this_query": delta,
        "hooked": hooked,
        "results": driven.get("results"),
        "attribution": attrib,
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
    elif driven.get("results") and attrib.get("attributable") is True:
        out["verdict"] = "FILTERED-AND-MATCHED"
        out["why"] = (
            f"the query carried our location filter and returned {driven['results']} "
            "lobby(ies), every one of which carries our key -- a host publishing at that location "
            "was found"
        )
    elif driven.get("results"):
        # The filter went out and lobbies are visible, but they are not this query's answer. Saying
        # so is the finding; calling it a match is how two false results got reported.
        out["verdict"] = "FILTERED-RESULTS-NOT-OURS"
        out["why"] = (
            f"our filter reached the wire, but the {driven['results']} visible lobby(ies) cannot be "
            f"this query's answer: {attrib['note']}. The transport is proven; the match is not"
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

    def matched(n, carrying):
        return {"ok": True, "results": n, "map_key": LOBBY_MAP_KEY,
                "carrying_map_key": [{"lobby": f"{i:x}", "value": "m12_02_00_00"}
                                     for i in range(carrying)]}

    v = verdict(matched(2, 2), counters(True, 0), counters(True, 1))
    check(v["verdict"] == "FILTERED-AND-MATCHED",
          "a filtered query whose every returned lobby carries our key is the end-to-end result")

    # THE DEFECT THIS FIXES, measured 2026-08-06: six lobbies came back, NONE carried the key, and
    # the verdict read FILTERED-AND-MATCHED twice. The result set was somebody else's query -- the
    # poll reads a shared slot -- and timing alone can never tell those apart.
    v = verdict(matched(6, 0), counters(True, 0), counters(True, 1))
    check(v["verdict"] == "FILTERED-RESULTS-NOT-OURS",
          "returned lobbies that lack our key are NOT this query's answer")
    check("transport is proven; the match is not" in v["why"],
          "and the report separates what was proven from what was not")

    v = verdict(matched(6, 4), counters(True, 0), counters(True, 1))
    check(v["verdict"] == "FILTERED-RESULTS-NOT-OURS",
          "a partially-carrying set is still unattributable -- an equality filter admits no exceptions")

    # Attribution is only decidable when the key was actually read back; an untested set must not
    # be reported as either attributable or not.
    v = verdict({"ok": True, "results": 3}, counters(True, 0), counters(True, 1))
    check(v["attribution"]["attributable"] is None and "not tested" in v["attribution"]["note"],
          "an untested attribution is unmeasured, never a measured 'no'")

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


#: How long to keep looking for the DLL to rewrite its oracle document, and how often to look.
#: The deadline is a BACKSTOP -- the loop returns the instant the document changes -- and it is
#: well under the repo's 30s non-game cap.
REPUBLISH_DEADLINE_SECONDS = 10.0
REPUBLISH_SAMPLE_SECONDS = 0.25


def wait_for_republish(path: str, before: dict) -> dict:
    """Read the oracle document once the DLL has rewritten it, rather than after a fixed delay.

    This replaced a flat `time.sleep(2.0)` whose comment was "give it one tick before sampling".
    That is synchronisation by guessing: 2s is simultaneously too long when the DLL republishes
    immediately and too short whenever a frame hitches, and a sample taken too early reads the
    PREVIOUS counters and reports "the detour declined to filter" for a query that filtered fine.

    The DLL rewrites the document edge-triggered, when a location-matchmaking counter moves, so
    the readiness condition is simply "the document is no longer what it was". Return as soon as
    that is true. The deadline exists only so a run where nothing ever changes still finishes, and
    returning the last read on expiry is correct: no change IS the observation in that case.
    """
    deadline = time.monotonic() + REPUBLISH_DEADLINE_SECONDS
    latest = before
    while time.monotonic() < deadline:
        latest = read_oracles(path)
        if latest != before:
            return latest
        # Sample period, not synchronisation: there is no notification when a file is rewritten by
        # another process, so the document is re-read at a fixed rate until it differs.
        time.sleep(REPUBLISH_SAMPLE_SECONDS)
    return latest


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
        ACCESSOR, args.own_lobby, args.poll_ms, args.poll_ticks, LOBBY_MAP_KEY
    )
    after = wait_for_republish(args.telemetry, before)

    result = {"driven": driven, "verdict": verdict(driven, before, after)}
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    with open(args.out, "w", encoding="utf-8") as handle:
        json.dump(result, handle, indent=2)
    print(json.dumps(result, indent=2))
    print(f"\nwrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
