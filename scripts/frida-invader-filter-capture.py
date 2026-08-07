#!/usr/bin/env python3
"""Capture the filters ersc attaches to an invasion query, and the keys the matching lobbies carry.

WHY
---
Every claim in this repo about "which lobbies an invader can reach" traces back to ONE capture of
ersc's filter set, whose VALUES were then treated as constants:

    lobby_breakin_lobby_ykssr_199_6      == "true"
    matchmaking_breakin_lobby_ykssr_199_6 == "4_3"
    lobby_type                           == "yknx3_seamless_master_lobby"
    ykssr_dlc                            == 1        (numerical)
    lobby_key                            == <sha256 of the password>

That model is FALSIFIED. Measured 2026-08-06: a host lobby reading
`lobby_breakin_lobby_ykssr_199_6 = 'false'` and `ykssr_dlc = '0'` was invaded within seconds of the
host opening to wanderers. A lobby the model calls unreachable was reached. Two of the three values
had already been seen varying (`4_3` vs `5_3`, `1` vs `0`) and were explained away as bracket/DLC
while `breakin` kept its constant status on no better evidence.

So stop inferring the semantics and MEASURE both halves at once:

  * what the query DEMANDS -- every key, value and comparison operator ersc attaches, and
  * what the lobbies that PASSED actually carry.

A returned lobby whose value contradicts a recorded filter means the model of that filter is wrong
-- most likely the comparison operator, which was never captured before and was assumed to be
equality. `ELobbyComparison` also has NotEqual (3) and the four ordering forms, and a NotEqual
filter reads exactly backwards from an equality one.

WHAT IT HOOKS, AND WHAT IT DELIBERATELY DOES NOT
------------------------------------------------
Attaches to vtable slots 5 (string filter), 6 (numerical filter) and 12 (GetLobbyByIndex) only.

It does NOT attach to slot 4 (`RequestLobbyList`) or slot 20 (`SetLobbyData`): the product DLL
already ilhook-detours both, and stacking a Frida trampoline on a live detour of a function this
path calls constantly is a crash risk taken for no information -- the query's firing is inferred
from the filter burst that always precedes it.

READ ONLY. Filters are logged, never added or altered; lobby keys are read with `GetLobbyData`.
Nothing is published, no session is started, no other player is affected.

    uv run --with frida python3 scripts/frida-invader-filter-capture.py
    python3 scripts/frida-invader-filter-capture.py --selftest
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import threading

GADGET = "127.0.0.1:27042"
ACCESSOR = "SteamAPI_SteamMatchmaking_v009"

#: `ELobbyComparison`, from the Steamworks SDK. Captured as an integer and named here rather than
#: assumed: the previous model silently took every filter for equality, and NotEqual (3) inverts
#: the meaning of a recorded key/value pair completely.
COMPARISON_NAMES = {
    -2: "EqualToOrLessThan",
    -1: "LessThan",
    0: "Equal",
    1: "GreaterThan",
    2: "EqualToOrGreaterThan",
    3: "NotEqual",
}

#: Keys worth reading back off every matching lobby. Seamless's seven observed keys plus ours; any
#: key a filter names is added to this set at run time, so a filter on something unanticipated is
#: still checked against what the lobby carries.
BASE_KEYS = (
    "lobby_type",
    "lobby_key",
    "lobby_preferences",
    "ykssr_dlc",
    "lobby_breakin_lobby_ykssr_199_6",
    "matchmaking_breakin_lobby_ykssr_199_6",
    "er_invasion_warp_map",
)

#: Our own key. Its presence on a lobby we did not publish to would mean another DLL user is out
#: there; its presence on lobbies generally is what the product depends on.
LOBBY_MAP_KEY = "er_invasion_warp_map"

#: A key no lobby publishes, used to answer the one question hunt's whole mechanism rests on: does
#: Steam EXCLUDE a lobby that lacks a filtered key? Namespaced so it cannot collide with a real key
#: and is obvious in a capture.
PROBE_KEY = "er_effects_probe_absent_key"
PROBE_VALUE = "1"

#: The filter ersc attaches LAST. Injection rides its return, so the probe filter joins the same
#: accumulated set that the imminent RequestLobbyList consumes -- without hooking RequestLobbyList,
#: which the product DLL already ilhook-detours.
LAST_FILTER_KEY = "lobby_key"


def _default_out() -> str:
    override = os.environ.get("ER_PROBE_OUT_DIR")
    if override:
        return os.path.join(override, "invader-filter-capture.jsonl")
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    return os.path.join(repo, "target", "steam-probe", "invader-filter-capture.jsonl")


AGENT = r"""
const SLOT_ADD_STRING_FILTER    = 5;
const SLOT_ADD_NUMERICAL_FILTER = 6;
const SLOT_GET_LOBBY_BY_INDEX   = 12;
const SLOT_GET_LOBBY_DATA       = 19;

let iface = null, vt = null, getByIndex = null, getData = null, addFilter = null;
let readKeys = [];

// Exclusion probe. When armed, a filter on a key NOBODY publishes is appended to alternating
// queries, so baseline and filtered are measured in the SAME session under the SAME conditions --
// far stronger than comparing two runs minutes apart, when the population changes underneath.
let probeKey = null, probeValue = null, probeMode = 'off';
let queryOrdinal = 0, injectedThisQuery = false;

// Filters accumulated since the last query completed. Steam CONSUMES the accumulated filter set
// when RequestLobbyList runs, so a burst of filter calls belongs to exactly one query.
let pending = [];

// SELF-CONTAMINATION GUARD. Our own result walk calls GetLobbyByIndex through the same hook that
// records ersc's pick. Without this, a run with one genuine pick logs hundreds of ours and any
// count derived from the indices is reading this probe's own out-of-range request. That exact
// mistake already produced a false measurement in this repo on 2026-08-06.
let inOurProbe = false;

const MAX_INDEX = 200;
const POLL_MS = 250, POLL_TICKS = 40;      // 10s window
const DEBOUNCE_MS = 400;                   // quiet gap after the last filter == the query fired

function slotFn(i) { return vt.add(i * Process.pointerSize).readPointer(); }

function collectResults() {
  if (getByIndex === null) return [];
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
        ids.push('0x' + id.toString(16));
      } catch (e) { break; }
    }
  } finally { inOurProbe = false; }
  return ids;
}

function readLobbyKeys(lobbyHex) {
  const values = {};
  const lobby = uint64(lobbyHex);
  for (const k of readKeys) {
    try {
      const r = getData(iface, lobby, Memory.allocUtf8String(k));
      // readCString, never readUtf8String(N): the length form THROWS when fewer than N bytes are
      // mapped, which silently turned every readable key into null in an earlier capture.
      values[k] = r.isNull() ? null : r.readCString();
    } catch (e) { values[k] = null; }
  }
  return values;
}

let pollTimer = null, debounceTimer = null;

function startPoll(filters, injected) {
  if (pollTimer !== null) return;
  let ticks = 0, best = [], firstSeen = {};
  pollTimer = setInterval(function () {
    ticks++;
    const ids = collectResults();
    // SAMPLE EACH LOBBY THE TICK IT FIRST APPEARS, never at the end of the window.
    //
    // The earlier version read every lobby's keys once, after the full 10s poll, "because reading
    // them every tick would multiply GetLobbyData traffic for no extra information". That was
    // wrong, and it produced false MODEL-FALSIFIED verdicts on 2026-08-06: `lobby_breakin` flips
    // as hosts open and close their worlds, so a key sampled 10s after a match was compared
    // against the filter values from before it -- and one host was recorded as violating a filter
    // it had satisfied when it actually matched. Observed directly: a single lobby read 'false',
    // 'false', 'true' across three queries seconds apart, and the host we invaded read 'false'
    // afterwards purely because our arrival had closed her world.
    //
    // Sampling on first sight bounds the skew to one poll interval instead of the whole window,
    // and each lobby is still read exactly once.
    for (let i = 0; i < ids.length; i++) {
      if (firstSeen[ids[i]] === undefined) {
        firstSeen[ids[i]] = { values: readLobbyKeys(ids[i]), tick: ticks };
      }
    }
    if (ids.length > best.length) best = ids;
    if (ticks >= POLL_TICKS) {
      clearInterval(pollTimer);
      pollTimer = null;
      const lobbies = best.map(function (id) {
        const rec = firstSeen[id];
        // A lobby with no first-sight record cannot happen (best is a subset of what was walked),
        // but read rather than emit nulls if it ever does -- and say which tick it came from, so a
        // late sample is visible in the artefact instead of being indistinguishable from a prompt one.
        return rec === undefined
          ? { lobby: id, values: readLobbyKeys(id), sampled_tick: null }
          : { lobby: id, values: rec.values, sampled_tick: rec.tick };
      });
      send({ type: 'query', filters: filters, results: best.length, lobbies: lobbies,
             probe_injected: injected, poll_ms: POLL_MS });
    }
  }, POLL_MS);
}

function armDebounce() {
  if (debounceTimer !== null) clearTimeout(debounceTimer);
  debounceTimer = setTimeout(function () {
    debounceTimer = null;
    const filters = pending;
    const injected = injectedThisQuery;
    pending = [];
    injectedThisQuery = false;
    send({ type: 'filters', filters: filters, probe_injected: injected });
    startPoll(filters, injected);
  }, DEBOUNCE_MS);
}

rpc.exports = {
  install: function (accessorName, keys, mode, pKey, pValue) {
    readKeys = keys;
    probeMode = mode; probeKey = pKey; probeValue = pValue;
    const mod = Process.enumerateModules().find(m => /steam_api(64)?\.dll/i.test(m.name)) || null;
    if (mod === null) return { ok: false, why: 'steam_api64.dll not loaded' };
    let acc = null;
    for (const e of mod.enumerateExports()) {
      if (e.type === 'function' && e.name === accessorName) { acc = e.address; break; }
    }
    if (acc === null) return { ok: false, why: 'accessor not exported' };
    try {
      iface = new NativeFunction(acc, 'pointer', [])();
      vt = iface.readPointer();
    } catch (e) { return { ok: false, why: 'interface unreadable: ' + e }; }

    try {
      getByIndex = new NativeFunction(slotFn(SLOT_GET_LOBBY_BY_INDEX), 'pointer',
                                      ['pointer', 'pointer', 'int']);
      getData = new NativeFunction(slotFn(SLOT_GET_LOBBY_DATA), 'pointer',
                                   ['pointer', 'uint64', 'pointer']);
      addFilter = new NativeFunction(slotFn(SLOT_ADD_STRING_FILTER), 'void',
                                     ['pointer', 'pointer', 'pointer', 'int']);
    } catch (e) { return { ok: false, why: 'cannot bind vtable slots: ' + e }; }

    // The interface POINTER varied across four values in one session, so hooks go on the VTABLE
    // SLOT ADDRESSES -- shared by every instance of the class -- and therefore catch ersc's calls
    // whatever object it holds.
    try {
      Interceptor.attach(slotFn(SLOT_ADD_STRING_FILTER), {
        onEnter(args) {
          let key = null, value = null;
          try { key = args[1].readCString(); } catch (e) { /* unreadable */ }
          try { value = args[2].readCString(); } catch (e) { /* unreadable */ }
          this.key = key;
          // Our own injected filter must not be recorded as one ersc attached, or the capture
          // would show the target demanding a key this probe invented.
          if (key !== probeKey) {
            pending.push({ kind: 'string', key: key, value: value,
                           comparison: args[3].toInt32(), this: args[0].toString() });
          }
          armDebounce();
        },
        onLeave() {
          // Steam ACCUMULATES filters and RequestLobbyList consumes the set, so appending here --
          // as ersc finishes staging its last one -- lands on the very next query. This is why the
          // probe needs no hook on RequestLobbyList itself, which the DLL already detours.
          if (probeMode === 'off' || this.key !== LAST_FILTER_KEY) return;
          if (injectedThisQuery) return;
          queryOrdinal++;
          // Alternate: the run produces its own control. Comparing an injected query against a
          // baseline taken minutes earlier would let the host population move between them, and a
          // drop to zero would then have two explanations.
          const wanted = probeMode === 'always' || (queryOrdinal % 2 === 0);
          if (!wanted) return;
          try {
            addFilter(iface, Memory.allocUtf8String(probeKey),
                      Memory.allocUtf8String(probeValue), 0);   // 0 == k_ELobbyComparisonEqual
            injectedThisQuery = true;
            send({ type: 'inject', ok: true, key: probeKey, ordinal: queryOrdinal });
          } catch (e) {
            send({ type: 'inject', ok: false, why: '' + e });
          }
        },
      });
      Interceptor.attach(slotFn(SLOT_ADD_NUMERICAL_FILTER), {
        onEnter(args) {
          let key = null;
          try { key = args[1].readCString(); } catch (e) { /* unreadable */ }
          pending.push({ kind: 'numerical', key: key, value: args[2].toInt32(),
                         comparison: args[3].toInt32(), this: args[0].toString() });
          armDebounce();
        },
      });
      Interceptor.attach(slotFn(SLOT_GET_LOBBY_BY_INDEX), {
        onEnter(args) {
          if (inOurProbe) return;
          send({ type: 'ersc-pick', index: args[2].toInt32() });
        },
      });
    } catch (e) { return { ok: false, why: 'attach failed: ' + e }; }

    return { ok: true, iface: iface.toString(), vtable: vt.toString(), keys: readKeys.length };
  },
};
"""


def satisfies(filt: dict, values: dict) -> bool | None:
    """Does a lobby's actual key value satisfy one captured filter?

    Returns None when it cannot be decided (unparsable numeric, unknown comparison) rather than
    guessing -- an undecidable check reported as a pass would manufacture agreement between the
    model and the measurement, which is precisely the error this tool exists to catch.
    """
    key = filt.get("key")
    if key is None:
        return None
    actual = values.get(key)
    op = filt.get("comparison")
    if filt.get("kind") == "numerical":
        try:
            actual_n = int(actual) if actual not in (None, "") else 0
            want_n = int(filt.get("value"))
        except (TypeError, ValueError):
            return None
        left, right = actual_n, want_n
    else:
        # Steam returns "" for a key a lobby does not carry, and that is what an equality filter
        # compares against -- absence is an empty string, not a wildcard.
        left, right = (actual or ""), (filt.get("value") or "")
    if op == 0:
        return left == right
    if op == 3:
        return left != right
    if op == -1:
        return left < right
    if op == -2:
        return left <= right
    if op == 1:
        return left > right
    if op == 2:
        return left >= right
    return None


def analyse(filters: list[dict], lobbies: list[dict]) -> dict:
    """Compare what the query demanded against what the matching lobbies actually carry.

    The finding that matters is a CONTRADICTION: a lobby Steam returned whose value fails a filter
    that same query attached. That cannot happen if the filter means what it was assumed to mean,
    so each one falsifies the recorded model of that key -- comparison operator, slot mapping, or
    which query the filters belonged to.
    """
    rows, contradictions = [], []
    for entry in lobbies:
        values = entry.get("values") or {}
        checks = []
        for filt in filters:
            ok = satisfies(filt, values)
            checks.append({
                "key": filt.get("key"),
                "wanted": filt.get("value"),
                "comparison": COMPARISON_NAMES.get(filt.get("comparison"), filt.get("comparison")),
                "actual": values.get(filt.get("key")),
                "satisfied": ok,
            })
            if ok is False:
                contradictions.append({
                    "lobby": entry.get("lobby"),
                    "key": filt.get("key"),
                    "wanted": filt.get("value"),
                    "comparison": COMPARISON_NAMES.get(filt.get("comparison"),
                                                       filt.get("comparison")),
                    "actual": values.get(filt.get("key")),
                })
        rows.append({"lobby": entry.get("lobby"), "checks": checks,
                     "carries_our_key": bool(values.get(LOBBY_MAP_KEY))})
    if not lobbies:
        verdict = "no-lobbies-returned"
        why = ("the query matched nothing, so nothing can be said about what a passing lobby "
               "carries -- this is 'nobody was there', not a finding about the filters")
    elif contradictions:
        verdict = "MODEL-FALSIFIED"
        why = (f"{len(contradictions)} returned lobby/filter pair(s) violate a filter this same "
               "query attached; the recorded meaning of those keys is wrong")
    else:
        verdict = "consistent"
        why = ("every returned lobby satisfies every captured filter, so the filter set explains "
               "the result set")
    return {
        "filters": filters,
        "lobby_count": len(lobbies),
        "rows": rows,
        "contradictions": contradictions,
        "carrying_our_key": [r["lobby"] for r in rows if r["carries_our_key"]],
        "verdict": verdict,
        "why": why,
    }


def exclusion_verdict(queries: list[dict]) -> dict:
    """Does Steam EXCLUDE a lobby that lacks a filtered key?

    Hunt's entire mechanism rests on yes. If a missing key instead PASSED, an equality filter on a
    block id would match every vanilla host, and the feature would look like it worked while
    narrowing nothing.

    Judged only from paired queries in one session -- some carrying a filter on a key nobody
    publishes, some not. A filtered zero is meaningless unless an unfiltered query in the same run
    found somebody: with nobody online, everything returns zero and calling that confirmation would
    manufacture the finding.
    """
    base = [q["results"] for q in queries if not q.get("probe_injected")]
    filt = [q["results"] for q in queries if q.get("probe_injected")]
    base_max = max(base, default=0)
    out = {"baseline_counts": base, "filtered_counts": filt, "baseline_best": base_max}
    if not filt:
        out["verdict"] = "no-filtered-query"
        out["why"] = "the probe filter never went out; nothing was tested"
    elif base_max == 0:
        out["verdict"] = "inconclusive-empty-baseline"
        out["why"] = (
            "no unfiltered query in this run found anybody, so a filtered zero is 'nobody was "
            "there' rather than evidence about exclusion"
        )
    elif max(filt) == 0:
        out["verdict"] = "CONFIRMED-missing-key-excludes"
        out["why"] = (
            f"unfiltered queries found up to {base_max} lobby(ies); every query carrying a filter "
            "on an unpublished key returned 0. A lobby without the key is excluded, so hunt's "
            "publish+filter mechanism is sound"
        )
    elif min(filt) >= base_max:
        out["verdict"] = "REFUTED-missing-key-passes"
        out["why"] = (
            f"the filter on an unpublished key changed nothing ({filt} against baseline "
            f"{base_max}). Steam does not exclude lobbies lacking the key, so a location filter "
            "would match everyone and hunt cannot work as designed"
        )
    else:
        out["verdict"] = "partial"
        out["why"] = (
            f"filtered {filt} against baseline {base_max}: narrowed but not to zero, which no "
            "model of an equality filter predicts -- needs another run"
        )
    return out


def _selftest() -> int:
    fails = 0

    def check(ok: bool, label: str) -> None:
        nonlocal fails
        print(f"  {'ok  ' if ok else 'FAIL'} {label}")
        if not ok:
            fails += 1

    eq = {"kind": "string", "key": "k", "value": "true", "comparison": 0}
    check(satisfies(eq, {"k": "true"}) is True, "an equality filter passes on the matching value")
    check(satisfies(eq, {"k": "false"}) is False, "and fails on a different value")

    # Absence is an empty string to Steam, never a wildcard. If a missing key passed, a location
    # filter would match every vanilla host and the whole design would silently invert.
    check(satisfies(eq, {}) is False, "a missing key fails an equality filter, it does not pass")

    # THE ASSUMPTION THAT WAS NEVER CHECKED. The old model read every captured key/value pair as
    # equality; NotEqual reverses which lobbies match, which alone could explain a 'false' host
    # being reachable.
    ne = dict(eq, comparison=3)
    check(satisfies(ne, {"k": "false"}) is True, "a NotEqual filter passes exactly where Equal fails")
    check(satisfies(ne, {"k": "true"}) is False, "and fails where Equal passes")

    num = {"kind": "numerical", "key": "ykssr_dlc", "value": 1, "comparison": 0}
    check(satisfies(num, {"ykssr_dlc": "1"}) is True, "a numerical filter parses the string value")
    check(satisfies(num, {"ykssr_dlc": "0"}) is False, "and rejects a different number")
    check(satisfies(num, {"ykssr_dlc": "abc"}) is None,
          "an unparsable numeric is undecidable, never silently a pass")
    check(satisfies({"kind": "string", "key": "k", "value": "x", "comparison": 99}, {"k": "x"})
          is None, "an unknown comparison operator is undecidable, never assumed to be equality")

    # A returned lobby that violates a filter the same query attached. This is the whole point:
    # the model of that key is wrong, and saying so is more useful than any tidy verdict.
    v = analyse(
        [dict(eq, key="lobby_breakin_lobby_ykssr_199_6")],
        [{"lobby": "0x1", "values": {"lobby_breakin_lobby_ykssr_199_6": "false"}}],
    )
    check(v["verdict"] == "MODEL-FALSIFIED", "a returned lobby violating a filter falsifies the model")
    check(v["contradictions"][0]["actual"] == "false", "and names the value that contradicts it")

    v = analyse(
        [dict(eq, key="lobby_type", value="yknx3_seamless_master_lobby")],
        [{"lobby": "0x1", "values": {"lobby_type": "yknx3_seamless_master_lobby",
                                     LOBBY_MAP_KEY: "m12_02_00_00"}}],
    )
    check(v["verdict"] == "consistent", "a fully satisfied result set is consistent")
    check(v["carrying_our_key"] == ["0x1"], "and lobbies carrying our key are reported")

    # An empty result set is not evidence about anything.
    v = analyse([eq], [])
    check(v["verdict"] == "no-lobbies-returned" and "not a finding" in v["why"],
          "no lobbies returned is reported as no evidence, not as agreement")

    # --- exclusion probe ---------------------------------------------------------------------
    def q(n, injected):
        return {"results": n, "probe_injected": injected}

    check(exclusion_verdict([q(1, False)])["verdict"] == "no-filtered-query",
          "with no injected query nothing was tested")

    # THE TRAP: filtered zero against an empty baseline is 'nobody online'. Reporting it as
    # confirmation would ship hunt on no evidence at all.
    v = exclusion_verdict([q(0, False), q(0, True)])
    check(v["verdict"] == "inconclusive-empty-baseline",
          "a filtered zero with an empty baseline is inconclusive, never confirmation")

    v = exclusion_verdict([q(1, False), q(0, True)])
    check(v["verdict"] == "CONFIRMED-missing-key-excludes",
          "a nonzero baseline and a filtered zero confirms exclusion")

    v = exclusion_verdict([q(1, False), q(1, True)])
    check(v["verdict"] == "REFUTED-missing-key-passes" and "cannot work as designed" in v["why"],
          "an unchanged count refutes exclusion and says hunt cannot work")

    check(exclusion_verdict([q(4, False), q(2, True)])["verdict"] == "partial",
          "a partial narrowing is reported as partial, not rounded toward either answer")

    if fails:
        print(f"selftest FAILED ({fails})")
        return 1
    print("selftest ok")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--gadget", default=GADGET)
    ap.add_argument("--out", default=_default_out())
    ap.add_argument("--seconds", type=float, default=0.0,
                    help="stop after N seconds; 0 runs until the script is stopped")
    ap.add_argument(
        "--probe-exclusion",
        choices=("off", "alternate", "always"),
        default="off",
        help="append a filter on an unpublished key to test whether Steam excludes lobbies that "
             "lack it. 'alternate' injects on every second query so the run carries its own "
             "baseline. While armed your search legitimately finds nobody on those queries -- that "
             "IS the expected result, and it narrows only your own outgoing query",
    )
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        return _selftest()

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
    done = threading.Event()
    queries: list[dict] = []

    def on_message(message, _data):
        if message.get("type") != "send":
            print(f"[agent] {message}", file=sys.stderr)
            return
        p = message.get("payload") or {}
        handle.write(json.dumps(p) + "\n")
        handle.flush()
        kind = p.get("type")
        if kind == "inject":
            if p.get("ok"):
                print(f"  + probe filter {p['key']} appended to query #{p['ordinal']}")
            else:
                print(f"  ! probe injection FAILED: {p.get('why')}")
        elif kind == "filters":
            tag = "  [PROBE ARMED]" if p.get("probe_injected") else ""
            print(f"\nQUERY -- {len(p['filters'])} filter(s) attached:{tag}")
            for f in p["filters"]:
                op = COMPARISON_NAMES.get(f.get("comparison"), f.get("comparison"))
                print(f"    {f['kind']:9s} {f.get('key')!r} {op} {f.get('value')!r}")
            print("  collecting results for 10s ...")
        elif kind == "ersc-pick":
            print(f"  ersc chose index {p['index']}")
        elif kind == "query":
            queries.append(p)
            report = analyse(p["filters"], p["lobbies"])
            print(f"  {p['results']} lobbies matched  [{report['verdict']}]")
            for row in report["rows"]:
                bad = [c for c in row["checks"] if c["satisfied"] is False]
                mark = "CONTRADICTS" if bad else "ok"
                print(f"    {row['lobby']}  {mark}"
                      f"{'  ours!' if row['carries_our_key'] else ''}")
                for c in row["checks"]:
                    flag = {True: " ", False: "!", None: "?"}[c["satisfied"]]
                    print(f"      {flag} {c['key']:38s} want {c['comparison']} {c['wanted']!r}"
                          f"  actual {c['actual']!r}")
            handle.write(json.dumps({"type": "analysis", **report}) + "\n")
            handle.flush()

    script.on("message", on_message)
    script.on("destroyed", done.set)
    script.load()
    print(json.dumps(script.exports_sync.install(
        ACCESSOR, list(BASE_KEYS), args.probe_exclusion, PROBE_KEY, PROBE_VALUE), indent=2))
    print(f"\nwriting {args.out}")
    print("Now run an INVASION search in game. Filters are logged as they are attached.\n")

    try:
        done.wait(timeout=args.seconds if args.seconds > 0 else None)
    except KeyboardInterrupt:
        pass
    finally:
        try:
            script.unload()
        except Exception:
            pass
        handle.close()

    print(f"\n{len(queries)} quer(ies) captured -> {args.out}")
    if not queries:
        print("NO QUERY OBSERVED. No invasion search ran while this was attached.", file=sys.stderr)
        return 0
    if args.probe_exclusion != "off":
        report = exclusion_verdict(queries)
        print("\n=== MISSING-KEY EXCLUSION ===")
        print(json.dumps(report, indent=2))
        with open(args.out, "a", encoding="utf-8") as handle:
            handle.write(json.dumps({"type": "exclusion", **report}) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
