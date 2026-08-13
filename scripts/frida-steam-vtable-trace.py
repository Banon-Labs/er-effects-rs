#!/usr/bin/env python3
"""Trace Seamless's Steam calls at the INTERFACE VTABLE, where they actually happen.

WHY THIS EXISTS
---------------
Hooking `steam_api64.dll`'s flat C exports found nothing. Measured 2026-08-04: 33 exports hooked
at BOOT -- 18 `ISteamMatchmaking` (every lobby filter/search/join/data call), 10
`ISteamNetworking*` (P2P and `SteamNetworkingMessages`), 5 `ISteamFriends` rich presence -- and a
complete user-driven invasion produced ZERO calls across all of them.

Simultaneous total silence on three unrelated surfaces has one clean explanation: the
`SteamAPI_ISteam*_*` exports are a C WRAPPER. A C++ mod obtains the interface pointer ONCE --
`SteamAPI_ISteamClient_GetISteamMatchmaking` is present in the export table -- and thereafter
calls VTABLE SLOTS on that object directly, never re-entering the flat exports.

So this hooks one level down: intercept the accessors to capture the returned interface POINTER,
then hook that object's vtable slots. Slot indices are reported rather than names, because the
slot ORDER is what the interface version pins -- and the version string is right there in the
accessor's argument, so a slot can be named later from the SDK header for that exact version
instead of guessed at now.

This does NOT assume Seamless uses matchmaking. It records whatever it calls. If the answer is
that no interface is used either, that is a real result and the next layer down is ERSC's own
socket code.

READ-ONLY: arguments are logged, never altered.

REACHING THE PROCESS
--------------------
Wine/Proton, so `frida.attach()` sees nothing. `frida-gadget.dll` loads into the game as an me3
`[[natives]]` entry and listens on 127.0.0.1:27042; connect to that as a REMOTE DEVICE. Use a
gadget-bearing profile, e.g. /home/banon/Elden/pr190-invasion-warp-seamless-frida.me3

ATTACH AT BOOT. An interface fetched during startup is invisible to a tracer that attaches after
it, and the accessors are typically called exactly once. That mistake already cost one run.

    uv run --with frida python3 /home/banon/projects/er-effects-rs/scripts/frida-steam-vtable-trace.py
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
    "fdd5f467-bf36-402d-bbcd-6defe1f4d0b7/scratchpad/steam-vtable-trace.jsonl"
)

#: How many vtable slots to hook per interface. ISteamMatchmaking is ~20 methods and
#: ISteamNetworkingSockets ~40; 64 covers every shipped interface with room to spare, and a slot
#: that is never called costs one idle hook.
DEFAULT_SLOTS = 64

AGENT = r"""
const captured = {};   // interface name -> { ptr, slots }
let slotBudget = 64;

function steamModule() {
  return Process.enumerateModules().find(m => /steam_api(64)?\.dll/i.test(m.name)) || null;
}

// A vtable slot is only worth hooking if it points at real code. A null or unreadable entry
// means the interface is shorter than the budget, which is the normal way to run off the end.
function plausibleCode(p) {
  try {
    if (p.isNull()) return false;
    const m = Process.findModuleByAddress(p);
    return m !== null;
  } catch (e) { return false; }
}

function hookVtable(label, obj) {
  let vt;
  try { vt = obj.readPointer(); } catch (e) { return 0; }
  let hooked = 0;
  for (let i = 0; i < slotBudget; i++) {
    let fn;
    try { fn = vt.add(i * Process.pointerSize).readPointer(); } catch (e) { break; }
    if (!plausibleCode(fn)) continue;
    try {
      const slot = i;
      Interceptor.attach(fn, {
        onEnter(args) {
          // args[0] is `this`. Capture the next few both raw and, where the bytes look like
          // printable text, as a string -- lobby keys and filter values are char*.
          const out = [];
          for (let a = 1; a < 5; a++) {
            let raw = '<?>', str = null;
            try { raw = args[a].toString(); } catch (err) { /* unreadable */ }
            try {
              const s = args[a].readUtf8String(96);
              if (s !== null && s.length > 0 && /^[\x20-\x7e]+$/.test(s)) str = s;
            } catch (err) { /* not a string */ }
            out.push({ raw: raw, str: str });
          }
          send({ type: 'vcall', iface: label, slot: slot, args: out });
        },
      });
      hooked++;
    } catch (e) { /* slot not attachable */ }
  }
  return hooked;
}

rpc.exports = {
  start: function (budget) {
    slotBudget = budget;
    const m = steamModule();
    if (m === null) return { error: 'steam_api not loaded' };
    const armed = [];
    for (const e of m.enumerateExports()) {
      if (e.type !== 'function') continue;
      // The accessors that HAND OUT an interface pointer. Both the ISteamClient_GetISteam*
      // family and the SteamInternal_* factories, since which one a mod uses varies.
      const isAccessor =
        /^SteamAPI_ISteamClient_GetISteam/.test(e.name) ||
        /^SteamInternal_(FindOrCreate|Create)/.test(e.name) ||
        /^SteamAPI_SteamMatchmaking/.test(e.name) ||
        /^SteamAPI_SteamNetworking/.test(e.name) ||
        /^SteamAPI_SteamFriends/.test(e.name);
      if (!isAccessor) continue;
      try {
        const label = e.name;
        Interceptor.attach(e.address, {
          onEnter(args) {
            // The interface VERSION string is what pins slot order; capture it so a slot index
            // can be resolved to a method name from the matching SDK header later.
            // NO FILTER. Two successive attempts to recognise the version string by shape both
            // returned null for every interface, so the identifying field stayed lost while the
            // trace looked like it worked. Capture the raw argument values AND their bytes and
            // decide what they are offline -- a filter applied at capture time can only throw
            // away what it failed to anticipate, and there is no second chance at a boot-time
            // call that happens once.
            this.version = null;
            this.rawArgs = [];
            for (let a = 0; a < 6; a++) {
              const rec = { i: a, raw: '<?>', utf8: null, utf16: null, bytes: null };
              try { rec.raw = args[a].toString(); } catch (err) { /* unreadable */ }
              // readUtf8String(N) reads EXACTLY N bytes and throws on the non-UTF8 data
              // past the terminator, which is why two shape-filters above it looked like
              // they were failing to match when the read itself was failing. readCString
              // stops at the NUL, which is what a char* argument actually is.
              try { rec.utf8 = args[a].readCString(); } catch (err) { /* not a string */ }
              try { rec.utf16 = args[a].readUtf16String(32); } catch (err) { /* not utf16 */ }
              try {
                const b = args[a].readByteArray(32);
                if (b !== null) {
                  rec.bytes = Array.from(new Uint8Array(b))
                    .map(function (x) { return ('0' + x.toString(16)).slice(-2); }).join('');
                }
              } catch (err) { /* unreadable */ }
              this.rawArgs.push(rec);
              if (this.version === null && rec.utf8 !== null &&
                  /^[A-Za-z][A-Za-z0-9_]{3,40}\d{3}$/.test(rec.utf8)) {
                this.version = rec.utf8;
              }
            }
          },
          onLeave(retval) {
            if (retval.isNull()) return;
            const key = (this.version || label) + '@' + retval.toString();
            if (captured[key] !== undefined) return;   // accessors are often called repeatedly
            const n = hookVtable(this.version || label, retval);
            captured[key] = n;
            send({
              type: 'iface', accessor: label, version: this.version,
              ptr: retval.toString(), slotsHooked: n, rawArgs: this.rawArgs,
            });
          },
        });
        armed.push(e.name);
      } catch (err) { /* not attachable */ }
    }
    return { accessorsArmed: armed.length, names: armed };
  },
};
"""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gadget", default=GADGET)
    parser.add_argument("--out", default=DEFAULT_OUT)
    parser.add_argument("--slots", type=int, default=DEFAULT_SLOTS)
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
            "ERROR: frida is not importable. uv provisions it per-run:\n"
            "  uv run --with frida python3 "
            "/home/banon/projects/er-effects-rs/scripts/frida-steam-vtable-trace.py",
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
    ifaces = 0
    calls = 0

    def on_message(message, _data):
        nonlocal ifaces, calls
        if message.get("type") != "send":
            print(f"[agent] {message}", file=sys.stderr)
            return
        payload = message.get("payload") or {}
        handle.write(json.dumps(payload) + "\n")
        handle.flush()
        if payload.get("type") == "iface":
            ifaces += 1
            print(
                f"INTERFACE {payload.get('version') or payload['accessor']} "
                f"ptr={payload['ptr']} slots_hooked={payload['slotsHooked']}"
            )
        elif payload.get("type") == "vcall":
            calls += 1
            shown = [f'"{a["str"]}"' if a.get("str") else a["raw"] for a in payload["args"]]
            print(f"{payload['iface']}::slot[{payload['slot']}]({', '.join(shown)})")

    script.on("message", on_message)
    script.load()
    print(script.exports_sync.start(args.slots))
    print(f"tracing -> {args.out}")
    print("Attach at BOOT. Interface accessors are usually called once, during startup.")

    done = threading.Event()
    script.on("destroyed", done.set)
    if sys.stdin.isatty():
        try:
            sys.stdin.read()
        except KeyboardInterrupt:
            pass
    else:
        print("detached: no tty, tracing until the game exits")
        done.wait()

    handle.close()
    print(f"{ifaces} interface(s) captured, {calls} vtable call(s) -> {args.out}")
    if ifaces == 0:
        print(
            "NO INTERFACE WAS EVER HANDED OUT while this tracer was attached. Either it attached "
            "too late (the accessors run once at startup) or Seamless does not obtain its Steam "
            "interfaces through steam_api64's accessors at all -- which would point at its own "
            "socket code as the next layer.",
            file=sys.stderr,
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
