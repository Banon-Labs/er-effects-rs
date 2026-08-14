#!/usr/bin/env python3
"""Find which field of Seamless's session object holds the ADVERTISED lobby.

WHY
---
`SESSION_LOBBY_ID_OFFSET = 0x178` was a candidate, and its own docstring said so. Measured
2026-08-06 while hosting: the lobby at that offset is ours, carries `lobby_type`, and accepts our
write -- but carries NONE of `lobby_breakin_lobby_ykssr_199_6`, `matchmaking_breakin_lobby_ykssr_199_6`
or `ykssr_dlc`. Every lobby a Seamless query can return carries those, because ersc's own five
filters demand them. So our key is going onto the member/session lobby, not the advertisement, and
no invader's query can ever see it.

Neither existing guard catches this: the owner check passes (we own both lobbies) and
`is_advertisement_lobby` passes (both carry `lobby_type`).

WHAT THIS DOES
--------------
Scans the session object for 8-byte values that look like Steam lobby ids, then asks Steam about
each one: do we own it, and does it carry the break-in trio? The right offset is the one whose
lobby answers yes to both. Everything here is a READ -- no writes, no session calls.

    uv run --with frida python3 scripts/frida-session-lobby-fields.py --session 0x471bcd10
    python3 scripts/frida-session-lobby-fields.py --selftest
"""

from __future__ import annotations

import argparse
import json
import sys

GADGET = "127.0.0.1:27042"
ACCESSOR = "SteamAPI_SteamMatchmaking_v009"

#: The keys that decide whether a lobby is reachable by a Seamless invasion query. Taken from the
#: captured filter set, not from guesswork: ersc attaches equality filters on all three.
ADVERTISEMENT_KEYS = (
    "lobby_breakin_lobby_ykssr_199_6",
    "matchmaking_breakin_lobby_ykssr_199_6",
    "ykssr_dlc",
)
LOBBY_TYPE_KEY = "lobby_type"
LOBBY_MAP_KEY = "er_invasion_warp_map"

#: Steam lobby CSteamIDs on this account all share a high nibble pattern; anything else in the
#: struct is a pointer, a float or a counter. Used only to SHORTLIST candidates -- every shortlisted
#: id is then confirmed by asking Steam about it, so a loose filter costs nothing.
LOBBY_ID_MASK = 0xFFF0_0000_0000_0000
LOBBY_ID_PREFIX = 0x0180_0000_0000_0000


def looks_like_lobby_id(value: int) -> bool:
    """Cheap shortlist test. Deliberately permissive; confirmation is done by Steam, not by this."""
    return value != 0 and (value & LOBBY_ID_MASK) == LOBBY_ID_PREFIX


AGENT = r"""
const SLOT_GET_LOBBY_DATA = 19;
const SLOT_GET_LOBBY_OWNER = 35;
const USER_GET_STEAM_ID_SLOT = 2;

rpc.exports = {
  scan: function (accessorName, sessionHex, span, advKeys, typeKey, mapKey) {
    const mod = Process.enumerateModules().find(m => /steam_api(64)?\.dll/i.test(m.name)) || null;
    if (mod === null) return { ok: false, why: 'steam_api64.dll not loaded' };
    let acc = null, userAcc = null;
    for (const e of mod.enumerateExports()) {
      if (e.type !== 'function') continue;
      if (e.name === accessorName) acc = e.address;
      if (/^SteamAPI_SteamUser_v\d+$/.test(e.name) && userAcc === null) userAcc = e.address;
    }
    if (acc === null) return { ok: false, why: 'matchmaking accessor not exported' };

    let iface, vt, me = null;
    try {
      iface = new NativeFunction(acc, 'pointer', [])();
      vt = iface.readPointer();
    } catch (e) { return { ok: false, why: 'interface unreadable: ' + e }; }
    try {
      const user = new NativeFunction(userAcc, 'pointer', [])();
      const uvt = user.readPointer();
      const get = new NativeFunction(uvt.add(USER_GET_STEAM_ID_SLOT * Process.pointerSize).readPointer(),
                                     'pointer', ['pointer', 'pointer']);
      const out = Memory.alloc(8); out.writeU64(0);
      get(user, out);
      me = '0x' + out.readU64().toString(16);
    } catch (e) { me = null; }

    const getData = new NativeFunction(vt.add(SLOT_GET_LOBBY_DATA * Process.pointerSize).readPointer(),
                                       'pointer', ['pointer', 'uint64', 'pointer']);
    const getOwner = new NativeFunction(vt.add(SLOT_GET_LOBBY_OWNER * Process.pointerSize).readPointer(),
                                        'pointer', ['pointer', 'pointer', 'uint64']);

    const session = ptr(sessionHex);
    const found = [];
    for (let off = 0; off < span; off += 8) {
      let raw;
      try { raw = session.add(off).readU64(); } catch (e) { continue; }
      const hex = raw.toString(16);
      // Shortlist by NUMERIC MASK, not by string shape. A lobby id renders as 15 hex digits, not
      // 16 -- `toString(16)` drops the leading zero of 0x0186… -- and a length-based regex silently
      // matched nothing at all, reporting "no lobby fields" for a session that plainly has one.
      if (raw.and(uint64('0xfff0000000000000')).toString(16) !== '180000000000000') continue;
      const lobby = uint64('0x' + hex);
      let owner = null, values = {};
      try {
        const out = Memory.alloc(8); out.writeU64(0);
        getOwner(iface, out, lobby);
        owner = '0x' + out.readU64().toString(16);
      } catch (e) { owner = null; }
      for (const k of advKeys.concat([typeKey, mapKey])) {
        try {
          const r = getData(iface, lobby, Memory.allocUtf8String(k));
          values[k] = r.isNull() ? null : r.readCString();
        } catch (e) { values[k] = null; }
      }
      found.push({ offset: off, lobby: '0x' + hex, owner: owner, values: values });
    }
    return { ok: true, local_user: me, fields: found };
  },

  // Raw window, unfiltered. Exists because the first version of the shortlist matched NOTHING and
  // reported "no lobby fields" for a session that demonstrably has one -- a filter that is wrong
  // and a struct that is empty look identical from the outside. This shows the bytes.
  dump: function (sessionHex, from, to) {
    const session = ptr(sessionHex);
    const out = [];
    for (let off = from; off < to; off += 8) {
      try {
        const raw = session.add(off).readU64();
        if (raw.compare(0) !== 0) out.push({ offset: off, value: '0x' + raw.toString(16) });
      } catch (e) { out.push({ offset: off, value: '<unreadable>' }); }
    }
    return out;
  },
};
"""


def classify(field: dict, me: str | None) -> dict:
    """Is this the lobby we should publish onto?

    Two conditions, both required. OURS, because only an owner's write persists. ADVERTISED,
    because ersc's own query filters on the break-in trio -- a lobby without them is invisible to
    every invader no matter what else it carries. `lobby_type` alone is NOT sufficient: both
    lobbies carry it, which is why the existing guard waved the wrong one through.
    """
    values = field.get("values") or {}
    ours = me is not None and field.get("owner") == me
    advertised = all(values.get(k) for k in ADVERTISEMENT_KEYS)
    typed = values.get(LOBBY_TYPE_KEY) == "yknx3_seamless_master_lobby"
    if ours and advertised:
        verdict = "PUBLISH-HERE"
    elif ours and typed:
        verdict = "ours-but-not-advertised"
    elif ours:
        verdict = "ours-but-not-a-seamless-lobby"
    else:
        verdict = "not-ours"
    return {
        "verdict": verdict,
        "ours": ours,
        "advertised": advertised,
        "carries_lobby_type": typed,
        "has_our_key": bool(values.get(LOBBY_MAP_KEY)),
    }


def _selftest() -> int:
    fails = 0

    def check(ok: bool, label: str) -> None:
        nonlocal fails
        print(f"  {'ok  ' if ok else 'FAIL'} {label}")
        if not ok:
            fails += 1

    me = "0x1100001018fa4be"
    adv = {k: v for k, v in zip(ADVERTISEMENT_KEYS, ("true", "4_3", "0"))}
    adv[LOBBY_TYPE_KEY] = "yknx3_seamless_master_lobby"

    check(classify({"owner": me, "values": adv}, me)["verdict"] == "PUBLISH-HERE",
          "ours and carrying the break-in trio is the publish target")

    # THE MEASURED BUG: lobby_type alone passed the old guard, and this is the lobby it chose.
    session_only = {LOBBY_TYPE_KEY: "yknx3_seamless_master_lobby", LOBBY_MAP_KEY: "m32_05_00_00"}
    v = classify({"owner": me, "values": session_only}, me)
    check(v["verdict"] == "ours-but-not-advertised",
          "a lobby with lobby_type but no break-in keys is NOT the advertisement")
    check(v["has_our_key"] and not v["advertised"],
          "and it is flagged as carrying our key while being unreachable -- the exact defect")

    check(classify({"owner": "0x11000012fd097f6", "values": adv}, me)["verdict"] == "not-ours",
          "a real host's advertisement is refused because we cannot write to it")

    # An empty string is Steam's answer for a key that is absent; it must not count as present.
    blank = dict(adv)
    blank["ykssr_dlc"] = ""
    check(not classify({"owner": me, "values": blank}, me)["advertised"],
          "an empty value is absence, not presence")

    if fails:
        print(f"selftest FAILED ({fails})")
        return 1
    print("selftest ok")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--session", help="Seamless session pointer, hex")
    ap.add_argument("--span", type=lambda v: int(v, 0), default=0x400)
    ap.add_argument("--gadget", default=GADGET)
    ap.add_argument("--dump", action="store_true")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        return _selftest()
    if not args.session:
        ap.error("--session is required")

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
    if args.dump:
        for row in script.exports_sync.dump(args.session, 0x100, 0x200):
            print(f"  +{row['offset']:#06x}  {row['value']}")
        return 0
    result = script.exports_sync.scan(
        ACCESSOR, args.session, args.span, list(ADVERTISEMENT_KEYS), LOBBY_TYPE_KEY, LOBBY_MAP_KEY
    )
    if not result.get("ok"):
        print(f"ERROR: {result.get('why')}", file=sys.stderr)
        return 4
    me = result.get("local_user")
    print(f"local user: {me}")
    for field in result["fields"]:
        verdict = classify(field, me)
        print(f"\n  +{field['offset']:#06x}  {field['lobby']}  [{verdict['verdict']}]")
        print(f"      owner {field['owner']}")
        for k, v in (field.get("values") or {}).items():
            print(f"      {k:38s} {v!r}")
    winners = [f for f in result["fields"] if classify(f, me)["verdict"] == "PUBLISH-HERE"]
    print(f"\npublish targets found: {[hex(f['offset']) for f in winners]}")
    print(json.dumps({"local_user": me, "publish_offsets": [f["offset"] for f in winners]}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
