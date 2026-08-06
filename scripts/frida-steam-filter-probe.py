#!/usr/bin/env python3
"""Does a Steam lobby WITHOUT a key fail an equality filter on that key?

WHY THIS ONE QUESTION IS WORTH A RUN
------------------------------------
The whole "request invasions by location" design rests on it. The plan is: a host publishes
`er_map = <block>` on its lobby, and an invader adds `AddRequestLobbyListStringFilter("er_map", ...)`
so only hosts at that place come back.

That works ONLY if a lobby lacking the key is EXCLUDED. If Steam instead treats a missing key as a
pass, the filter matches every vanilla host, the design silently inverts, and the feature would
appear to work while doing nothing at all. It is documented behaviour, not measured behaviour, and
this repo has been burned before by shipping a predicate that could never fire.

WHAT IT DOES
------------
Two searches, same session:

  BASELINE  (default)         count how many lobbies the real query returns
  FILTERED  (--inject-filter) add one filter on a key NOBODY publishes, then count again

Zero-with-a-nonzero-baseline confirms exclusion. Unchanged counts refute it.

MEASURED 2026-08-06, and why counting needs care: ersc calls `GetLobbyByIndex` exactly ONCE per
query, and it asked for **index 14** -- so the result set had at least 15 entries and ersc is
choosing among them, not taking the first. Its single call therefore says nothing about how many
came back. The count here is taken by probing indices ourselves.

Results arrive ASYNCHRONOUSLY via a callback that lives inside ersc's Themida-virtualized region,
so there is nothing readable to hook for "results are ready". Instead the probe polls for a bounded
window after `RequestLobbyList` and records the high-water mark. That distinguishes the three cases
that matter: results arrived and were counted, results never arrived within the window, and the
probe never ran at all.

THE ONE THING THAT MODIFIES BEHAVIOUR: `--inject-filter` adds a filter to YOUR OWN outgoing query.
It narrows what you see and nothing else -- no other player is affected, nothing is published, and
no game state is written. It is off by default, and while armed your search will legitimately find
nobody if the hypothesis is right. That IS the expected result.

    uv run --with frida python3 scripts/frida-steam-filter-probe.py --iface 0x45d1bb50
    uv run --with frida python3 scripts/frida-steam-filter-probe.py --iface 0x45d1bb50 --inject-filter
    python3 scripts/frida-steam-filter-probe.py --selftest
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import threading

GADGET = "127.0.0.1:27042"


def _default_out() -> str:
    """Where the capture lands, without baking one machine's session directory into the tool.

    The previous default was an absolute path containing a specific user, a specific uid and a
    specific agent session id. It worked exactly once, in the session that wrote it: any later run
    -- another user, another session, the same user after a reboot -- would have written into a
    directory that no longer meant anything. `ER_PROBE_OUT_DIR` overrides; otherwise the capture
    goes next to the repo's other run artefacts, found by walking up from this file rather than
    assuming a checkout location.
    """
    override = os.environ.get("ER_PROBE_OUT_DIR")
    if override:
        return os.path.join(override, "steam-filter-probe.jsonl")
    here = os.path.dirname(os.path.abspath(__file__))
    repo = os.path.dirname(here)
    return os.path.join(repo, "target", "steam-probe", "steam-filter-probe.jsonl")


DEFAULT_OUT = _default_out()

#: A key no Seamless host publishes. Deliberately namespaced so it cannot collide with a real key
#: if Seamless ever adds one, and so it is obvious in a capture that it came from this probe.
PROBE_KEY = "er_effects_probe_absent_key"
PROBE_VALUE = "1"

AGENT = r"""
const SLOT_REQUEST_LOBBY_LIST = 4;
const SLOT_ADD_STRING_FILTER  = 5;
const SLOT_GET_LOBBY_BY_INDEX = 12;

// ISteamMatchmaking is a C++ interface: `this` in RCX, and an 8-byte CSteamID return uses a hidden
// return pointer. Confirmed from GetLobbyMemberByIndex's observed (sret, lobby, index) shape.
let iface = null, vt = null;
let addFilter = null, getByIndex = null;
let injecting = false, probeKey = null, probeValue = null;
let armedFilters = 0;
// Our own lobby id, lower-case hex WITHOUT the 0x, or null when the caller did not supply one.
// Compared against the returned set so "did my own lobby come back" is a measured field rather
// than something read off a list by eye.
let ownLobby = null;

// Bounded so a bogus count can never spin: a Seamless query has never been observed returning
// anything near this, and an unbounded walk inside a live process is its own hazard.
const MAX_INDEX = 200;
// Results arrive async and the callback is inside the Themida region, so there is nothing to hook
// for "ready". Poll for a bounded window and keep the high-water mark.
const POLL_MS = 250, POLL_TICKS = 40;   // 10 seconds

function slotFn(i) { return vt.add(i * Process.pointerSize).readPointer(); }

// SELF-CONTAMINATION GUARD. Our own collectResults() calls go through the SAME GetLobbyByIndex hook
// that records ersc's choice, so without this every probe iteration was logged as an 'ersc-pick'.
// Measured 2026-08-06: one real pick (index 7) was followed by ~550 of our own, and the derived
// "ersc asked for index 13, so there are >=14 results" was reading OUR probe's own final
// out-of-range request. An instrument that cannot tell its own calls from the target's produces
// numbers that look like measurements and are not.
let inOurProbe = false;

// WHICH lobbies are currently in the result set. Probes indices until Steam returns the nil
// CSteamID, which is what an out-of-range index yields.
//
// The IDENTITIES are kept, not just the count, because a count cannot answer the question a
// single-machine run turns on: does a query return YOUR OWN lobby? If it does, a host that
// publishes a location key and then hunts for that same location proves the whole loop -- publish,
// filter, match -- without a second player. If it does not, a solo filtered search is zero by
// construction and can only ever prove the filter went out. The first version of this probe
// recorded indices alone and could not tell those two worlds apart.
function collectResults() {
  if (getByIndex === null) return { n: -1, ids: [] };
  const out = Memory.alloc(8);
  const ids = [];
  inOurProbe = true;
  try {
    for (let i = 0; i < MAX_INDEX; i++) {
      try {
        out.writeU64(0);
        getByIndex(iface, out, i);
        const id = out.readU64();
        if (id.compare(0) === 0) break;
        ids.push(id.toString(16));
      } catch (e) { break; }
    }
  } finally { inOurProbe = false; }
  return { n: ids.length, ids: ids };
}

let polling = false;
function pollAfterQuery(tag) {
  if (polling) return;
  polling = true;
  let ticks = 0, best = 0, bestIds = [];
  const t = setInterval(function () {
    ticks++;
    const got = collectResults();
    // Keep the identities from the SAME tick as the high-water count, so the list and the number
    // can never describe two different moments.
    if (got.n > best) { best = got.n; bestIds = got.ids; }
    if (ticks >= POLL_TICKS) {
      clearInterval(t);
      polling = false;
      send({ type: 'count', tag: tag, results: best, ticks: ticks,
             window_ms: POLL_MS * POLL_TICKS, filter_injected: injecting,
             filters_added: armedFilters, lobbies: bestIds,
             own_lobby: ownLobby,
             own_lobby_returned: ownLobby === null ? null : bestIds.indexOf(ownLobby) >= 0 });
    }
  }, POLL_MS);
}

rpc.exports = {
  install: function (ifacePtr, inject, key, value, own) {
    iface = ptr(ifacePtr);
    injecting = !!inject; probeKey = key; probeValue = value;
    ownLobby = own ? own.toLowerCase().replace(/^0x/, '') : null;
    try { vt = iface.readPointer(); } catch (e) { return { ok: false, why: 'interface unreadable' }; }

    try {
      getByIndex = new NativeFunction(slotFn(SLOT_GET_LOBBY_BY_INDEX), 'pointer',
                                      ['pointer', 'pointer', 'int']);
      addFilter = new NativeFunction(slotFn(SLOT_ADD_STRING_FILTER), 'void',
                                     ['pointer', 'pointer', 'pointer', 'int']);
    } catch (e) { return { ok: false, why: 'cannot bind vtable slots: ' + e }; }

    Interceptor.attach(slotFn(SLOT_REQUEST_LOBBY_LIST), {
      onEnter() {
        // Filters are accumulated then CONSUMED by RequestLobbyList, so adding one here -- before
        // the original body runs -- still lands on this query.
        if (injecting) {
          try {
            const k = Memory.allocUtf8String(probeKey);
            const v = Memory.allocUtf8String(probeValue);
            addFilter(iface, k, v, 0);   // 0 == k_ELobbyComparisonEqual
            armedFilters++;
            send({ type: 'inject', ok: true, key: probeKey, value: probeValue });
          } catch (e) {
            send({ type: 'inject', ok: false, why: '' + e });
          }
        }
        send({ type: 'query', injected: injecting });
        pollAfterQuery(injecting ? 'filtered' : 'baseline');
      },
    });

    // ersc's own pick. Recorded because the INDEX it chooses is a lower bound on the result set
    // independent of our polling, and because its absence means no results came back at all.
    Interceptor.attach(slotFn(SLOT_GET_LOBBY_BY_INDEX), {
      onEnter(args) {
        if (inOurProbe) return;   // our own counting walk, not ersc's choice
        send({ type: 'ersc-pick', index: args[2].toInt32() });
      },
    });

    return { ok: true, vtable: vt.toString(), injecting: injecting,
             probe_key: injecting ? probeKey : null, own_lobby: ownLobby };
  },
};
"""


def own_lobby_finding(counts: list[dict]) -> dict:
    """Does an unfiltered query return the querying player's OWN lobby?

    This decides whether location matchmaking can be proven on ONE machine. If Steam returns your
    own lobby, a host that publishes its map and then hunts for that same map matches itself, and
    publish -> filter -> match is established end to end with nobody else involved. If it does not,
    a solo filtered search is empty by construction and can only ever show that the filter left the
    process -- which is worth knowing, but is not the same claim.

    Reported as `unmeasured` unless the caller passed `--own-lobby`, because an absent field and a
    measured "no" are different answers and only one of them is evidence.
    """
    unfiltered = [c for c in counts if not c.get("filter_injected")]
    answered = [c for c in unfiltered if c.get("own_lobby_returned") is not None]
    if not answered:
        return {
            "own_lobby_returned": None,
            "own_lobby_note": "unmeasured -- pass --own-lobby <id> to settle whether a solo run "
            "can prove a match",
        }
    seen = any(c["own_lobby_returned"] for c in answered)
    return {
        "own_lobby_returned": seen,
        "own_lobby_note": (
            "an unfiltered query DOES return this player's own lobby, so hunting for the map this "
            "same client publishes proves publish->filter->match on one machine"
            if seen
            else "an unfiltered query does NOT return this player's own lobby, so a solo filtered "
            "search is empty by construction -- a second machine is required to prove a match, "
            "and a zero result here is not evidence against the filter"
        ),
    }


def verdict(records: list[dict]) -> dict:
    """What the run establishes about missing-key filter semantics.

    Deliberately refuses to conclude from a silent run: a filtered search returning zero is only
    meaningful against a baseline that returned something. Absence of results when nobody was
    online proves nothing, and reporting it as confirmation would be inventing a finding.
    """
    counts = [r for r in records if r.get("type") == "count"]
    base = [c["results"] for c in counts if not c.get("filter_injected")]
    filt = [c["results"] for c in counts if c.get("filter_injected")]
    picks = [r["index"] for r in records if r.get("type") == "ersc-pick"]
    # ersc's own chosen index is an independent lower bound: asking for index N means at least
    # N+1 results existed, regardless of what our polling managed to catch.
    implied = max(picks) + 1 if picks else 0
    base_max = max(base + ([implied] if implied else []), default=0)

    out = {
        "baseline_counts": base,
        "filtered_counts": filt,
        "ersc_picked_indices": picks,
        "implied_min_results": implied,
        "baseline_best": base_max,
    }
    out.update(own_lobby_finding(counts))
    if not base and not filt:
        out["verdict"] = "no-search-observed"
        out["why"] = "no lobby query ran; nothing was measured"
    elif base_max == 0:
        out["verdict"] = "inconclusive-empty-baseline"
        out["why"] = (
            "the unfiltered query returned nothing, so a filtered zero would prove nothing -- "
            "this is 'nobody was online', not evidence about filter semantics"
        )
    elif not filt:
        out["verdict"] = "baseline-only"
        out["why"] = f"baseline of {base_max} lobbies established; run again with --inject-filter"
    elif max(filt) == 0:
        out["verdict"] = "CONFIRMED-missing-key-excludes"
        out["why"] = (
            f"baseline returned {base_max} lobbies, the same query with a filter on an unpublished "
            "key returned 0 -- a lobby without the key is excluded, so publish+filter works"
        )
    elif min(filt) >= base_max:
        out["verdict"] = "REFUTED-missing-key-passes"
        out["why"] = (
            f"the filter on an unpublished key changed nothing ({filt} vs baseline {base_max}) -- "
            "Steam does not exclude lobbies lacking the key, so the location filter would match "
            "everyone and the design must change"
        )
    else:
        out["verdict"] = "partial"
        out["why"] = f"filtered {filt} vs baseline {base_max} -- narrowed but not to zero; needs another run"
    return out


def _selftest() -> int:
    fails = 0

    def check(ok: bool, label: str) -> None:
        nonlocal fails
        print(f"  {'ok  ' if ok else 'FAIL'} {label}")
        if not ok:
            fails += 1

    def count(n, injected):
        return {"type": "count", "results": n, "filter_injected": injected}

    check(verdict([])["verdict"] == "no-search-observed", "no search is not a finding")

    # THE TRAP THIS GUARDS. A filtered zero against an empty baseline is 'nobody online', and
    # calling it confirmation would ship a design on no evidence at all.
    v = verdict([count(0, False), count(0, True)])
    check(v["verdict"] == "inconclusive-empty-baseline",
          "filtered zero with an empty baseline is inconclusive, never confirmation")

    check(verdict([count(15, False)])["verdict"] == "baseline-only",
          "a baseline alone says so and asks for the second run")

    check(
        verdict([count(15, False), count(0, True)])["verdict"] == "CONFIRMED-missing-key-excludes",
        "nonzero baseline then filtered zero confirms exclusion",
    )
    check(
        verdict([count(15, False), count(15, True)])["verdict"] == "REFUTED-missing-key-passes",
        "an unchanged count refutes exclusion -- the design would invert",
    )
    check(
        verdict([count(15, False), count(7, True)])["verdict"] == "partial",
        "a partial narrowing is reported as partial, not rounded to either answer",
    )

    # ersc's chosen index is an independent lower bound on the result set, so a baseline can be
    # established even if our polling caught nothing.
    v = verdict([{"type": "ersc-pick", "index": 14}, count(0, False), count(0, True)])
    check(v["implied_min_results"] == 15 and v["verdict"] == "CONFIRMED-missing-key-excludes",
          "ersc picking index 14 proves >=15 results even when polling missed them")

    # The contamination this tool shipped with on its first run: our own counting walk went through
    # the same hook and every iteration logged as an 'ersc-pick', so a run with ONE real pick looked
    # like hundreds and the implied minimum was read off our own out-of-range probe. The agent now
    # suppresses its own calls; this pins the arithmetic that consumes them.
    v = verdict([{"type": "ersc-pick", "index": 7}, count(13, False)])
    check(v["implied_min_results"] == 8 and v["baseline_best"] == 13,
          "one genuine pick at index 7 implies >=8, and the counted 13 is the better baseline")

    # --- does a query return your own lobby? ---------------------------------------------------
    #
    # THE TRAP: reporting an unmeasured field as False. "we did not look" and "we looked and it was
    # not there" lead to opposite decisions -- one says get a second machine, the other says the
    # experiment is still open -- and a bare False conflates them.
    v = verdict([count(13, False)])
    check(v["own_lobby_returned"] is None and "unmeasured" in v["own_lobby_note"],
          "not passing --own-lobby reports unmeasured, never a measured 'no'")

    def owned(n, injected, present):
        return {"type": "count", "results": n, "filter_injected": injected,
                "own_lobby_returned": present}

    v = verdict([owned(13, False, True)])
    check(v["own_lobby_returned"] is True and "one machine" in v["own_lobby_note"],
          "own lobby present means a solo run can prove the whole loop")

    v = verdict([owned(13, False, False)])
    check(v["own_lobby_returned"] is False and "second machine" in v["own_lobby_note"],
          "own lobby absent means a solo filtered zero is not evidence against the filter")

    # Only the UNFILTERED query answers this. A filtered query that excluded our own lobby would
    # have excluded it on the filter, not on Steam's own-lobby policy, so reading the answer off
    # the filtered run would confuse the two.
    v = verdict([owned(13, False, True), owned(0, True, False)])
    check(v["own_lobby_returned"] is True,
          "the answer is taken from the unfiltered query, not from the filtered one")

    if fails:
        print(f"selftest FAILED ({fails})")
        return 1
    print("selftest ok")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--iface", help="ISteamMatchmaking pointer, e.g. 0x45d1bb50")
    ap.add_argument("--inject-filter", action="store_true",
                    help="add a filter on an unpublished key to YOUR OWN query (off by default)")
    ap.add_argument("--key", default=PROBE_KEY)
    ap.add_argument("--own-lobby",
                    help="this client's own lobby id (hex, e.g. 0x1860000164a8cd9). Settles "
                         "whether a query returns your own lobby, and therefore whether location "
                         "matchmaking can be proven without a second machine")
    ap.add_argument("--gadget", default=GADGET)
    ap.add_argument("--out", default=DEFAULT_OUT)
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        return _selftest()
    if not args.iface:
        ap.error("--iface is required")

    try:
        sys.stdout.reconfigure(line_buffering=True)
    except (AttributeError, ValueError):
        pass
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
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    handle = open(args.out, "w", encoding="utf-8")
    records: list[dict] = []
    done = threading.Event()

    def on_message(message, _data):
        if message.get("type") != "send":
            print(f"[agent] {message}", file=sys.stderr)
            return
        p = message.get("payload") or {}
        records.append(p)
        handle.write(json.dumps(p) + "\n")
        handle.flush()
        kind = p.get("type")
        if kind == "query":
            print(f"QUERY sent (filter injected: {p.get('injected')}) -- counting for 10s")
        elif kind == "count":
            own = p.get("own_lobby_returned")
            tail = "" if own is None else f"  own lobby returned: {own}"
            print(f"RESULT SET: {p['results']} lobbies  [{p['tag']}]{tail}")
        elif kind == "ersc-pick":
            print(f"ersc chose index {p['index']}  (implies >= {p['index']+1} results)")
        elif kind == "inject":
            print(f"{'INJECTED' if p.get('ok') else 'INJECT FAILED'} {p.get('key','')} {p.get('why','')}")

    script.on("message", on_message)
    script.on("destroyed", done.set)
    script.load()
    print(json.dumps(script.exports_sync.install(
        args.iface, args.inject_filter, args.key, PROBE_VALUE, args.own_lobby), indent=2))
    print(f"\nwriting {args.out}\nRun an invasion search, then Ctrl-C.\n")

    try:
        done.wait()
    except KeyboardInterrupt:
        pass
    finally:
        try:
            script.unload()
        except Exception:
            pass
        handle.close()

    print("\n=== VERDICT ===")
    print(json.dumps(verdict(records), indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
