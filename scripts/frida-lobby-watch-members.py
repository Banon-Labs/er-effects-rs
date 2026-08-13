#!/usr/bin/env python3
"""Watch a lobby's member count while hosting, to measure that a NON-DLL invader still finds us.

WHAT THIS SETTLES
-----------------
Publishing `er_invasion_warp_map` ADDS a key to the host's Steam lobby. The claim that this costs a
host nothing -- that a vanilla Seamless invader, whose five filters never name our key, still
matches -- follows from how equality filters work but has never been observed from the other side.
Self-exclusion (Steam omits your own lobby from your own query) makes it unobservable from one
seat... unless somebody else does the observing FOR us by actually invading.

That is what this watches. The DLL population is one person, so ANY invader who arrives is a
non-DLL invader by construction. An arrival, while the lobby demonstrably carries our key, is the
measurement: the extra key did not make this host invisible.

The pairing matters. `members` alone proves someone joined; `our key present` alone proves we
published. Only both at the same instant say a host carrying our key was reachable, so they are
sampled together and reported together.

READ ONLY: GetNumLobbyMembers / GetLobbyMemberByIndex / GetLobbyData. Nothing is written, no
session is started, no match is affected.

    uv run --with frida python3 scripts/frida-lobby-watch-members.py --lobby 0x186000016bc4229
    python3 scripts/frida-lobby-watch-members.py --selftest
"""

from __future__ import annotations

import argparse
import json
import sys
import time

GADGET = "127.0.0.1:27042"
ACCESSOR = "SteamAPI_SteamMatchmaking_v009"
LOBBY_MAP_KEY = "er_invasion_warp_map"

#: `ISteamMatchmaking` vtable slots. 4/5/12/19/20/35 are each independently verified against this
#: build (live captures + argument shapes), and 17/18 sit in the same published ordering between
#: verified neighbours. A wrong slot here fails loudly -- a bogus member count or a fault -- rather
#: than silently, and it writes nothing either way.
SLOT_NUM_LOBBY_MEMBERS = 17
SLOT_LOBBY_MEMBER_BY_INDEX = 18


def _default_out() -> str:
    import os

    override = os.environ.get("ER_PROBE_OUT_DIR")
    if override:
        return os.path.join(override, "lobby-members.jsonl")
    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    return os.path.join(repo, "target", "steam-probe", "lobby-members.jsonl")


AGENT = r"""
const SLOT_NUM_MEMBERS = 17;
const SLOT_MEMBER_BY_INDEX = 18;
const SLOT_GET_LOBBY_DATA = 19;

let iface = null, vt = null;

rpc.exports = {
  attach: function (accessorName) {
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
    return { ok: true, iface: iface.toString() };
  },

  sample: function (lobbyHex, key) {
    if (vt === null) return { ok: false, why: 'not attached' };
    const lobby = uint64(lobbyHex);
    let members = -1, ids = [], value = null;
    try {
      const num = new NativeFunction(vt.add(SLOT_NUM_MEMBERS * Process.pointerSize).readPointer(),
                                     'int', ['pointer', 'uint64']);
      members = num(iface, lobby);
    } catch (e) { return { ok: false, why: 'GetNumLobbyMembers threw: ' + e }; }
    try {
      // CSteamID by value -> hidden sret pointer, the convention this build uses throughout.
      const byIndex = new NativeFunction(
        vt.add(SLOT_MEMBER_BY_INDEX * Process.pointerSize).readPointer(),
        'pointer', ['pointer', 'pointer', 'uint64', 'int']);
      const out = Memory.alloc(8);
      for (let i = 0; i < Math.min(members, 16); i++) {
        out.writeU64(0);
        byIndex(iface, out, lobby, i);
        ids.push('0x' + out.readU64().toString(16));
      }
    } catch (e) { ids = ['<threw: ' + e + '>']; }
    try {
      const get = new NativeFunction(vt.add(SLOT_GET_LOBBY_DATA * Process.pointerSize).readPointer(),
                                     'pointer', ['pointer', 'uint64', 'pointer']);
      const r = get(iface, lobby, Memory.allocUtf8String(key));
      value = r.isNull() ? null : r.readCString();
    } catch (e) { value = '<threw: ' + e + '>'; }
    return { ok: true, members: members, member_ids: ids, key_value: value };
  },
};
"""


def classify(sample: dict, baseline: int | None) -> dict:
    """What one sample means for the compatibility claim.

    `baseline` is the member count while hosting alone. An arrival is a count ABOVE it, and it
    only counts as evidence when our key is present in the SAME sample -- a join observed while
    the key was missing says nothing about whether the key costs a host reach.
    """
    members = sample.get("members", -1)
    key = sample.get("key_value")
    has_key = bool(key)
    if members < 0:
        return {"state": "unreadable", "note": "member count could not be read"}
    if baseline is None:
        return {
            "state": "baseline",
            "note": f"hosting alone at {members} member(s); arrivals are counts above this",
        }
    if members <= baseline:
        return {
            "state": "waiting",
            "note": f"{members} member(s), no arrival yet; key {'present' if has_key else 'ABSENT'}",
        }
    if not has_key:
        return {
            "state": "arrival-without-key",
            "note": "somebody joined, but our key was NOT on the lobby at that moment -- this "
            "says nothing about whether publishing costs a host reach",
        }
    return {
        "state": "ARRIVAL-WITH-KEY",
        "note": f"{members} member(s), up from {baseline}, while the lobby carried "
        f"{LOBBY_MAP_KEY} = {key!r}. A non-DLL invader reached a host publishing our key: the "
        "extra key does not make a host invisible",
    }


def _selftest() -> int:
    fails = 0

    def check(ok: bool, label: str) -> None:
        nonlocal fails
        print(f"  {'ok  ' if ok else 'FAIL'} {label}")
        if not ok:
            fails += 1

    check(classify({"members": 1, "key_value": "m28_00_00_00"}, None)["state"] == "baseline",
          "the first sample establishes the alone-count rather than claiming an arrival")
    check(classify({"members": 1, "key_value": "m28_00_00_00"}, 1)["state"] == "waiting",
          "an unchanged count is not an arrival")

    # THE TRAP: counting a join that happened while the key was absent. That is a vanilla
    # invasion of a vanilla-looking host and proves nothing about publishing.
    v = classify({"members": 2, "key_value": ""}, 1)
    check(v["state"] == "arrival-without-key",
          "a join with no key present is explicitly NOT evidence")
    check("says nothing" in v["note"], "and says so rather than staying quiet")

    v = classify({"members": 2, "key_value": "m28_00_00_00"}, 1)
    check(v["state"] == "ARRIVAL-WITH-KEY", "a join while the key is live is the measurement")
    check("does not make a host invisible" in v["note"], "and states what it establishes")

    check(classify({"members": -1}, 1)["state"] == "unreadable",
          "an unreadable count is not silently treated as zero")

    if fails:
        print(f"selftest FAILED ({fails})")
        return 1
    print("selftest ok")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--lobby", help="the lobby to watch, hex")
    ap.add_argument("--key", default=LOBBY_MAP_KEY)
    ap.add_argument("--interval", type=float, default=2.0)
    ap.add_argument("--seconds", type=float, default=300.0)
    ap.add_argument("--gadget", default=GADGET)
    ap.add_argument("--out", default=_default_out())
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        return _selftest()
    if not args.lobby:
        ap.error("--lobby is required")

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
    script.load()
    attached = script.exports_sync.attach(ACCESSOR)
    if not attached.get("ok"):
        print(f"ERROR: {attached.get('why')}", file=sys.stderr)
        return 4

    import os

    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    handle = open(args.out, "w", encoding="utf-8")
    baseline: int | None = None
    deadline = time.monotonic() + args.seconds
    last_state = None
    while time.monotonic() < deadline:
        sample = script.exports_sync.sample(args.lobby, args.key)
        if not sample.get("ok"):
            print(f"sample failed: {sample.get('why')}", file=sys.stderr)
            break
        verdict = classify(sample, baseline)
        record = {"sample": sample, "verdict": verdict}
        handle.write(json.dumps(record) + "\n")
        handle.flush()
        if verdict["state"] != last_state or verdict["state"] == "ARRIVAL-WITH-KEY":
            print(f"[{verdict['state']}] {verdict['note']}")
            print(f"    members={sample['members']} ids={sample.get('member_ids')}")
            last_state = verdict["state"]
        if baseline is None:
            baseline = sample["members"]
        time.sleep(args.interval)
    print(f"\nwrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
