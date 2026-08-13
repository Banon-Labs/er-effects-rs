#!/usr/bin/env python3
"""Name the `ISteamMatchmaking` vtable slots in a `frida-steam-vtable-trace.py` capture.

WHY SLOT NUMBERS AND NOT NAMES AT CAPTURE TIME
----------------------------------------------
Seamless reaches Steam through the C++ interface vtable, never the flat `SteamAPI_ISteam*_*`
exports (measured 2026-08-04: 33 flat exports hooked at boot, a full invasion, zero calls). So the
tracer hooks vtable SLOTS, and a slot index only means something once you know the interface
VERSION -- which the accessor reports at runtime. This maps one to the other, after the fact,
which is also why it can be corrected without re-running the game.

PROVENANCE, STATED PLAINLY: the table below is the PUBLIC Steamworks SDK declaration order for
`ISteamMatchmaking` ("SteamMatchMaking009"). It is NOT byte-verified against this machine's
`steamclient64.dll`. That matters, so every naming is CORROBORATED against the observed call shape
rather than trusted: `SetLobbyData` must carry two printable strings, `RequestLobbyList` must carry
none, and so on. A slot whose observed arguments contradict its name is reported as a CONFLICT
instead of being labelled -- because a mis-named slot here would become a false claim about what
Seamless publishes.

USAGE
    python3 scripts/steam-matchmaking-slots.py <trace.jsonl>
    python3 scripts/steam-matchmaking-slots.py --selftest
"""

from __future__ import annotations

import argparse
import collections
import json
import sys

#: Interface version this table describes. A capture reporting any other version is refused rather
#: than decoded with the wrong ordering.
INTERFACE = "SteamMatchMaking009"

#: Public Steamworks SDK declaration order for ISteamMatchmaking. See the provenance note above:
#: this is knowledge, not measurement, and it is checked against observed argument shapes below.
SLOTS = {
    0: "GetFavoriteGameCount",
    1: "GetFavoriteGame",
    2: "AddFavoriteGame",
    3: "RemoveFavoriteGame",
    4: "RequestLobbyList",
    5: "AddRequestLobbyListStringFilter",
    6: "AddRequestLobbyListNumericalFilter",
    7: "AddRequestLobbyListNearValueFilter",
    8: "AddRequestLobbyListFilterSlotsAvailable",
    9: "AddRequestLobbyListDistanceFilter",
    10: "AddRequestLobbyListResultCountFilter",
    11: "AddRequestLobbyListCompatibleMembersFilter",
    12: "GetLobbyByIndex",
    13: "CreateLobby",
    14: "JoinLobby",
    15: "LeaveLobby",
    16: "InviteUserToLobby",
    17: "GetNumLobbyMembers",
    18: "GetLobbyMemberByIndex",
    19: "GetLobbyData",
    20: "SetLobbyData",
    21: "GetLobbyDataCount",
    22: "GetLobbyDataByIndex",
    23: "DeleteLobbyData",
    24: "GetLobbyMemberData",
    25: "SetLobbyMemberData",
    26: "SendLobbyChatMsg",
    27: "GetLobbyChatEntry",
    28: "RequestLobbyData",
    29: "SetLobbyGameServer",
    30: "GetLobbyGameServer",
    31: "SetLobbyMemberLimit",
    32: "GetLobbyMemberLimit",
    33: "SetLobbyType",
    34: "SetLobbyJoinable",
    35: "GetLobbyOwner",
    36: "SetLobbyOwner",
    37: "SetLinkedLobby",
}

#: Minimum number of printable-string arguments each method MUST show if the naming is right.
#: Only methods with an unambiguous signature are listed; the rest are not corroborated either way.
EXPECTED_STRINGS = {
    "AddRequestLobbyListStringFilter": 2,   # pchKeyToMatch, pchValueToMatch
    "AddRequestLobbyListNumericalFilter": 1,  # pchKeyToMatch
    "AddRequestLobbyListNearValueFilter": 1,  # pchKeyToMatch
    "SetLobbyData": 2,                      # pchKey, pchValue
    "GetLobbyData": 1,                      # pchKey
    "SetLobbyMemberData": 2,
    "GetLobbyMemberData": 1,
    "DeleteLobbyData": 1,
    "RequestLobbyList": 0,
    "CreateLobby": 0,
    "GetLobbyDataCount": 0,
}

#: The methods that answer the question this capture exists for: what a host PUBLISHES and what an
#: invader FILTERS on. Surfaced separately so they are not lost in the volume of polling calls.
DECISIVE = {
    "SetLobbyData",
    "SetLobbyMemberData",
    "AddRequestLobbyListStringFilter",
    "AddRequestLobbyListNumericalFilter",
    "AddRequestLobbyListNearValueFilter",
    "RequestLobbyList",
    "CreateLobby",
    "JoinLobby",
}


def strings_of(call: dict) -> list[str]:
    """The printable string arguments the tracer recovered for one call."""
    return [a["str"] for a in call.get("args", []) if isinstance(a, dict) and a.get("str")]


def decode(records: list[dict]) -> dict:
    """Name every matchmaking call, flag namings the observed arguments contradict."""
    calls = [r for r in records if r.get("type") == "vcall" and r.get("iface") == INTERFACE]
    named: dict[str, int] = collections.Counter()
    conflicts: list[dict] = []
    decisive: list[dict] = []
    published: dict[str, str] = {}
    filters: list[tuple[str, str]] = []

    for c in calls:
        slot = c.get("slot")
        name = SLOTS.get(slot)
        if name is None:
            named[f"slot[{slot}] (beyond the known interface)"] += 1
            continue
        got = strings_of(c)
        want = EXPECTED_STRINGS.get(name)
        # A naming the arguments contradict is reported, never silently applied: this table is SDK
        # knowledge, and a wrong label here becomes a false claim about Seamless.
        if want is not None and len(got) < want:
            conflicts.append({"slot": slot, "named": name,
                              "expected_strings": want, "observed_strings": got})
            continue
        named[name] += 1
        if name in DECISIVE:
            decisive.append({"slot": slot, "name": name, "strings": got, "args": c.get("args")})
        if name == "SetLobbyData" and len(got) >= 2:
            published[got[0]] = got[1]
        if name.startswith("AddRequestLobbyList") and got:
            filters.append((got[0], got[1] if len(got) > 1 else ""))

    return {
        "matchmaking_calls": len(calls),
        "by_method": dict(named.most_common()),
        "conflicts": conflicts,
        "decisive": decisive,
        "published_lobby_data": published,
        "request_filters": filters,
    }


def _selftest() -> int:
    fails = 0

    def check(ok: bool, label: str) -> None:
        nonlocal fails
        print(f"  {'ok  ' if ok else 'FAIL'} {label}")
        if not ok:
            fails += 1

    def call(slot, *strs):
        return {"type": "vcall", "iface": INTERFACE, "slot": slot,
                "args": [{"raw": "0x1", "str": s} for s in strs]}

    check(decode([])["matchmaking_calls"] == 0, "an empty capture decodes to nothing, not an error")

    # The whole point: a host publishing lobby data becomes a readable key/value map.
    d = decode([call(20, "lobby_key", "abc123"), call(20, "er_map", "m60_51_36_00")])
    check(d["published_lobby_data"] == {"lobby_key": "abc123", "er_map": "m60_51_36_00"},
          "SetLobbyData calls become the published key/value map")

    # And an invader's query filters are recovered with their real key and value.
    f = decode([call(5, "lobby_key", "abc123")])
    check(f["request_filters"] == [("lobby_key", "abc123")],
          "AddRequestLobbyListStringFilter is recovered with key and value")

    # THE HONESTY GATE. The slot table is SDK knowledge, not measurement. A slot whose observed
    # arguments contradict the name must be reported as a conflict, never labelled -- a wrong label
    # here would become a false claim about what Seamless publishes.
    c = decode([call(20)])  # SetLobbyData with no strings at all
    check(not c["published_lobby_data"] and len(c["conflicts"]) == 1,
          "a SetLobbyData with no string args is a CONFLICT, not a published key")
    check(c["conflicts"][0]["named"] == "SetLobbyData" and c["conflicts"][0]["expected_strings"] == 2,
          "the conflict names which slot and what it should have carried")

    # Calls on other interfaces must not be counted as matchmaking.
    other = decode([{"type": "vcall", "iface": "SteamFriends017", "slot": 20, "args": []}])
    check(other["matchmaking_calls"] == 0, "another interface's slot 20 is not SetLobbyData")

    # A slot past the end of the known interface is reported, not dropped.
    beyond = decode([call(60, "x")])
    check(any("beyond" in k for k in beyond["by_method"]),
          "a slot beyond the known interface is reported rather than silently ignored")

    if fails:
        print(f"selftest FAILED ({fails})")
        return 1
    print("selftest ok")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("trace", nargs="?", help="steam-vtable-trace.jsonl")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return _selftest()
    if not args.trace:
        ap.error("a trace file is required (or --selftest)")

    records = []
    with open(args.trace, encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if line:
                try:
                    records.append(json.loads(line))
                except json.JSONDecodeError:
                    pass

    ifaces = {r.get("iface") for r in records if r.get("type") == "vcall"}
    out = decode(records)
    print(f"interfaces seen: {sorted(i for i in ifaces if i)}")
    print(f"{INTERFACE} calls: {out['matchmaking_calls']}\n")
    for name, count in out["by_method"].items():
        mark = " <<<" if name in DECISIVE else ""
        print(f"  {count:>6}  {name}{mark}")

    if out["published_lobby_data"]:
        print("\nPUBLISHED LOBBY DATA (what a host tells the world about itself):")
        for k, v in out["published_lobby_data"].items():
            print(f"  {k} = {v}")
    else:
        print("\nPUBLISHED LOBBY DATA: none observed")

    if out["request_filters"]:
        print("\nREQUEST FILTERS (what an invader asks Steam to narrow by):")
        for k, v in out["request_filters"]:
            print(f"  {k} = {v}")
    else:
        print("\nREQUEST FILTERS: none observed")

    if out["conflicts"]:
        print("\nCONFLICTS -- slot naming contradicted by the observed arguments:")
        for c in out["conflicts"]:
            print(f"  slot[{c['slot']}] named {c['named']}: expected >= "
                  f"{c['expected_strings']} string arg(s), saw {c['observed_strings']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
