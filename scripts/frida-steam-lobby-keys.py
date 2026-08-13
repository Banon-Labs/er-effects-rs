#!/usr/bin/env python3
"""Capture the lobby KEYS and VALUES Seamless publishes and filters on, correctly.

WHY A SECOND TRACER RATHER THAN A FIX TO THE FIRST
--------------------------------------------------
`frida-steam-vtable-trace.py` finds the `ISteamMatchmaking` pointer by hooking the accessor that
hands it out -- and that accessor fires ONCE, at startup. Restarting it mid-session to fix a bug
would lose the pointer for the rest of the run. So this attaches to an ALREADY-KNOWN interface
pointer instead, which the first tracer prints on capture, and hooks the same vtable directly.
Both can run at once; neither needs the other to stop.

THE BUG IT EXISTS TO AVOID: `readUtf8String(96)` THROWS when fewer than 96 bytes are mapped, so a
short string near the end of a page reads as `null` and a published key silently disappears. That
is exactly what happened on the 2026-08-06 host run -- every `SetLobbyData` logged `str: null`
while the key bytes were visibly sitting in a register (`0x61705f7962626f6c` == "lobby_ap"). Use
`readCString()`, which stops at the NUL. See bd `frida-readutf8string-length-trap-use-readcstring`.

Strings are recovered three ways, because on this path each fails differently:
  1. as a `char*` (the declared signature)
  2. as INLINE BYTES in the register itself -- a small-string value passed by register, which is
     how "lobby_ap" showed up in arg4 and is invisible to any pointer-based read
  3. as a `char**` one level down, for a std::string-shaped argument

SLOT NAMES are the public Steamworks order for `SteamMatchMaking009`; see
`scripts/steam-matchmaking-slots.py` for the provenance caveat and the argument-shape cross-check.

READ-ONLY: arguments are logged, never altered, and nothing is called.

    uv run --with frida python3 scripts/frida-steam-lobby-keys.py --iface 0x45d1bb50
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
    "fdd5f467-bf36-402d-bbcd-6defe1f4d0b7/scratchpad/steam-lobby-keys.jsonl"
)

#: The slots worth the cost of a string read. Publishing and filtering are the whole question;
#: the polling getters would bury them.
WATCH = {
    4: "RequestLobbyList",
    5: "AddRequestLobbyListStringFilter",
    6: "AddRequestLobbyListNumericalFilter",
    7: "AddRequestLobbyListNearValueFilter",
    # GetLobbyByIndex is how a caller walks the RESULTS of a lobby-list query, so counting it is
    # the only way to learn how many candidates a query actually returned. That number decides
    # whether preferring among results is even possible: one result means there is nothing to
    # prefer, and the only lever is narrowing the request itself.
    12: "GetLobbyByIndex",
    13: "CreateLobby",
    14: "JoinLobby",
    19: "GetLobbyData",
    20: "SetLobbyData",
    21: "GetLobbyDataCount",
    22: "GetLobbyDataByIndex",
    23: "DeleteLobbyData",
    24: "GetLobbyMemberData",
    25: "SetLobbyMemberData",
    33: "SetLobbyType",
}

AGENT = r"""
let watch = {};

// A register may BE the string rather than point at it (small-string optimisation puts up to 8
// bytes straight in the register). That is invisible to every pointer-based read, and it is how
// the first run lost every key it captured.
function inlineAscii(nptr) {
  try {
    const v = nptr.toString(16).padStart(16, '0');
    let s = '';
    for (let i = 14; i >= 0; i -= 2) {
      const b = parseInt(v.substr(i, 2), 16);
      if (b === 0) break;
      if (b < 0x20 || b > 0x7e) return null;
      s += String.fromCharCode(b);
    }
    return s.length >= 2 ? s : null;
  } catch (e) { return null; }
}

// readCString stops at the NUL. readUtf8String(N) tries to read N bytes and THROWS when fewer are
// mapped -- which silently turns a real key into null.
function asString(nptr) {
  try {
    const s = nptr.readCString();
    if (s && s.length > 0 && /^[\x20-\x7e]+$/.test(s)) return s;
  } catch (e) { /* not a readable char* */ }
  return null;
}

function readArg(nptr) {
  const out = { raw: nptr.toString() };
  out.str = asString(nptr);
  out.inline = inlineAscii(nptr);
  if (out.str === null) {
    // std::string-shaped: the buffer may be one indirection down.
    try {
      const inner = nptr.readPointer();
      out.deref = asString(inner);
    } catch (e) { /* not a pointer */ }
  }
  return out;
}

rpc.exports = {
  install: function (ifacePtr, slots) {
    watch = slots;
    const obj = ptr(ifacePtr);
    let vt;
    try { vt = obj.readPointer(); } catch (e) { return { ok: false, why: 'interface not readable' }; }
    const hooked = [];
    Object.keys(watch).forEach(function (k) {
      const slot = parseInt(k, 10);
      let fn;
      try { fn = vt.add(slot * Process.pointerSize).readPointer(); } catch (e) { return; }
      if (fn.isNull()) return;
      try {
        Interceptor.attach(fn, {
          onEnter(args) {
            const a = [];
            // args[0] is `this`; a 64-bit CSteamID is passed by value and eats one slot, so read
            // generously rather than assuming where the strings sit.
            for (let i = 1; i <= 5; i++) {
              try { a.push(readArg(args[i])); } catch (e) { a.push({ raw: '<?>' }); }
            }
            send({ type: 'lobby', slot: slot, name: watch[slot], args: a });
          },
        });
        hooked.push(watch[slot]);
      } catch (e) { /* not attachable */ }
    });
    return { ok: true, vtable: vt.toString(), hooked: hooked };
  },
};
"""


def texts(arg: dict) -> list[str]:
    """Every string this argument yielded, by any of the three recovery routes."""
    return [v for v in (arg.get("str"), arg.get("inline"), arg.get("deref")) if v]


def summarize(records: list[dict]) -> dict:
    """The answer: what is published, and what is filtered on."""
    published: list[tuple[str, str]] = []
    filters: list[tuple[str, str]] = []
    seen_keys: set[str] = set()
    for r in records:
        if r.get("type") != "lobby":
            continue
        strs = [t for a in r.get("args", []) for t in texts(a)]
        name = r.get("name", "")
        if name in ("SetLobbyData", "SetLobbyMemberData") and strs:
            published.append((strs[0], strs[1] if len(strs) > 1 else ""))
            seen_keys.add(strs[0])
        elif name.startswith("AddRequestLobbyList") and strs:
            filters.append((strs[0], strs[1] if len(strs) > 1 else ""))
            seen_keys.add(strs[0])
    return {"published": published, "filters": filters, "keys": sorted(seen_keys)}


def _selftest() -> int:
    fails = 0

    def check(ok: bool, label: str) -> None:
        nonlocal fails
        print(f"  {'ok  ' if ok else 'FAIL'} {label}")
        if not ok:
            fails += 1

    def arg(s=None, inline=None, deref=None):
        return {"raw": "0x1", "str": s, "inline": inline, "deref": deref}

    check(summarize([])["published"] == [], "an empty capture publishes nothing, not an error")
    check(
        summarize([{"type": "lobby", "name": "SetLobbyData",
                    "args": [arg(), arg("er_map"), arg("m60_51_36_00")]}])["published"]
        == [("er_map", "m60_51_36_00")],
        "a char* key/value pair is recovered",
    )
    # THE CASE THAT MOTIVATED THIS TOOL: the pointer read fails, but the bytes are in the register.
    check(
        summarize([{"type": "lobby", "name": "SetLobbyData",
                    "args": [arg(), arg(inline="lobby_ap"), arg(inline="1245620")]}])["published"]
        == [("lobby_ap", "1245620")],
        "a key recovered from INLINE register bytes is not lost",
    )
    check(
        summarize([{"type": "lobby", "name": "SetLobbyData",
                    "args": [arg(), arg(deref="lobby_key"), arg(deref="abc")]}])["published"]
        == [("lobby_key", "abc")],
        "a std::string-shaped key one indirection down is recovered",
    )
    check(
        summarize([{"type": "lobby", "name": "AddRequestLobbyListStringFilter",
                    "args": [arg("lobby_key"), arg("abc123")]}])["filters"]
        == [("lobby_key", "abc123")],
        "a request filter is recovered with key and value",
    )
    # A call whose strings never resolved must NOT become a bogus published key.
    check(
        summarize([{"type": "lobby", "name": "SetLobbyData", "args": [arg(), arg(), arg()]}])["published"] == [],
        "a call with no recoverable strings publishes nothing rather than an empty key",
    )
    if fails:
        print(f"selftest FAILED ({fails})")
        return 1
    print("selftest ok")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--iface", help="ISteamMatchmaking pointer, e.g. 0x45d1bb50")
    ap.add_argument("--gadget", default=GADGET)
    ap.add_argument("--out", default=DEFAULT_OUT)
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        return _selftest()
    if not args.iface:
        ap.error("--iface is required (the first tracer prints it on capture)")

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
        strs = [t for a in p.get("args", []) for t in texts(a)]
        print(f"{p.get('name')}({', '.join(repr(s) for s in strs) or '...'})")

    script.on("message", on_message)
    script.on("destroyed", done.set)
    script.load()
    print(json.dumps(script.exports_sync.install(args.iface, {str(k): v for k, v in WATCH.items()}), indent=2))
    print(f"\nwriting {args.out}\n")

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

    out = summarize(records)
    print("\n=== PUBLISHED ===")
    for k, v in out["published"]:
        print(f"  {k} = {v}")
    print("=== FILTERS ===")
    for k, v in out["filters"]:
        print(f"  {k} = {v}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
