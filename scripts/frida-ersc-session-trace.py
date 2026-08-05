#!/usr/bin/env python3
"""Watch Seamless's session state machine and its option menu, live.

WHAT THIS ANSWERS
-----------------
Static RE got as far as it can. The option table, the menu builder and the confirm hook are all
plaintext and fully read (2026-08-04), but two things live inside the Themida-virtualized
`seamless_session_manager` dispatcher and cannot be read at all:

  * what puts the session into state 0x15 -- the ONLY state in which "Seek opponent" appears; and
  * what consumes the opponent handle that "Seek opponent" latches.

Neither is readable, but both are *observable*: the state is a plain dword and the latch is a
plain qword, so sampling them across a real session reconstructs the machine from the outside.

WHAT IT WATCHES, and why each field
-----------------------------------
Everything hangs off ERSC's option-menu object, `OSM`, and its session object `S = OSM+0x58`:

  S+0x110  session state.  0 -> only "Invade world as a wanderer" is offered;
                           0x15 -> "Seek opponent" (index 0) + "Mark world" (index 1);
                           0x0D/0x0E/0x0F/0x11 -> only "Cancel search";
                           anything else -> the menu will not open at all.
           "Invade world as a wanderer" sets it to 0x0D; "Cancel search" sets 0x22.
           NOTHING PLAINTEXT sets 0x15 -- that is the question.
  S+0x1D4  cleared to 0 by the "Seek opponent" action. One writer, zero plaintext readers.
  S+0x1F0  the CHOSEN OPPONENT'S HANDLE, latched by "Seek opponent". Zero plaintext consumers.
           This is the closest thing in the mod to "invade this specific person".

OSM is a heap allocation with no export, so it is captured rather than computed: the first call
to show() or to the confirm callback hands it over in RCX.

RVAs (module-base relative -- ersc.dll is RELOCATABLE, there is no fixed load address, so every
address is resolved against Process.findModuleByName('ersc.dll').base at runtime):
    0x22D30  show(void* OSM, int groupId)   prologue 55 41 57 41 56 41 55 41 54 56 57 53 48 81 EC
    0x806E0  confirm callback(ErscCtx* ctx, GameDialog* dlg); selected index at dlg+0xB0C
    0x243E0  action: "Invade world as a wanderer"   (sets S+0x110 = 0x0D)
    0x24BC0  action: "Seek opponent"                (latches S+0x1F0, clears S+0x1D4)
    0x24D10  action: "Mark world for other invaders"
    0x24460  action: "Cancel search"                (sets S+0x110 = 0x22)

READ-ONLY. Nothing is written and nothing in ERSC is called. The prologue of show() is verified
before hooking, so a version drift fails loudly instead of hooking the middle of some other
function.

REACHING THE PROCESS
--------------------
Wine/Proton: frida.attach() sees nothing. frida-gadget.dll is loaded into the game as an me3
[[natives]] entry and listens on 127.0.0.1:27042; connect to it as a REMOTE DEVICE. Use a
gadget-bearing profile, e.g. /home/banon/Elden/pr190-invasion-warp-seamless-frida.me3

    uv run --with frida python3 /home/banon/projects/er-effects-rs/scripts/frida-ersc-session-trace.py
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
    "fdd5f467-bf36-402d-bbcd-6defe1f4d0b7/scratchpad/ersc-session-trace.jsonl"
)

#: Session states whose meaning is established, for readable output. Anything absent is printed
#: as a bare number rather than guessed at -- an unknown state is the interesting case here.
STATE_NAMES = {
    0x00: "idle (only 'Invade world as a wanderer' offered)",
    0x0D: "searching (set by 'Invade world as a wanderer')",
    0x15: "SEEK-ELIGIBLE ('Seek opponent' + 'Mark world' offered)",
    0x22: "cancelled (set by 'Cancel search')",
}

AGENT = r"""
const RVA = {
  show:    0x22d30,
  confirm: 0x806e0,
  invade:  0x243e0,
  seek:    0x24bc0,
  mark:    0x24d10,
  cancel:  0x24460,
};
// show()'s first bytes, from static RE. Verified before hooking so a version drift fails loudly
// instead of silently attaching to whatever now lives at that offset.
const SHOW_PROLOGUE = '554157415641554154565753';

let base = null;
let osm = null;
let poll = null;
let last = null;

function hex(p) { try { return p.toString(); } catch (e) { return '<?>'; } }

function readSession() {
  if (osm === null) return null;
  try {
    const S = osm.add(0x58).readPointer();
    if (S.isNull()) return null;
    return {
      S: S.toString(),
      state: S.add(0x110).readU32(),
      f1d4: S.add(0x1d4).readU8(),
      latch: S.add(0x1f0).readU64().toString(),
    };
  } catch (e) { return null; }
}

function emitIfChanged(why) {
  const now = readSession();
  if (now === null) return;
  const key = JSON.stringify(now);
  if (key === last) return;
  last = key;
  send({ type: 'session', why: why, fields: now });
}

function captureOsm(p, why) {
  if (osm !== null || p === null || p.isNull()) return;
  osm = p;
  send({ type: 'osm', ptr: p.toString(), via: why });
  // Sample from here on: the state machine is driven from inside the VM, so transitions arrive
  // with no observable call of our own to hang a hook on.
  poll = setInterval(function () { emitIfChanged('poll'); }, 100);
  emitIfChanged('captured');
}

rpc.exports = {
  start: function () {
    const m = Process.findModuleByName('ersc.dll');
    if (m === null) return { error: 'ersc.dll not loaded' };
    base = m.base;

    const showAddr = base.add(RVA.show);
    let prologue = '';
    try {
      prologue = Array.from(new Uint8Array(showAddr.readByteArray(12)))
        .map(function (x) { return ('0' + x.toString(16)).slice(-2); }).join('');
    } catch (e) { /* unreadable */ }
    if (prologue !== SHOW_PROLOGUE) {
      return { error: 'show() prologue mismatch', expected: SHOW_PROLOGUE, got: prologue,
               base: base.toString() };
    }

    Interceptor.attach(showAddr, {
      onEnter(args) {
        captureOsm(args[0], 'show');
        let group = -1;
        try { group = args[1].toInt32(); } catch (e) { /* unreadable */ }
        send({ type: 'menu-open', group: group, osm: hex(args[0]) });
        emitIfChanged('menu-open');
      },
    });

    Interceptor.attach(base.add(RVA.confirm), {
      onEnter(args) {
        // args[0] = ErscCtx (OSM lives at +0x58 of it), args[1] = the game's dialog.
        let idx = -1;
        try { idx = args[1].add(0xb0c).readU32(); } catch (e) { /* unreadable */ }
        let ctxOsm = null;
        try { ctxOsm = args[0].add(0x58).readPointer(); } catch (e) { /* unreadable */ }
        captureOsm(ctxOsm, 'confirm-ctx');
        send({ type: 'confirm', selectedIndex: idx });
        emitIfChanged('confirm');
      },
    });

    [['invade', RVA.invade], ['seek', RVA.seek], ['mark', RVA.mark], ['cancel', RVA.cancel]]
      .forEach(function (pair) {
        Interceptor.attach(base.add(pair[1]), {
          onEnter(args) {
            captureOsm(args[0], 'action-' + pair[0]);
            send({ type: 'action', name: pair[0] });
            emitIfChanged('action-enter:' + pair[0]);
          },
          onLeave() { emitIfChanged('action-leave:' + pair[0]); },
        });
      });

    return { base: base.toString(), hooked: Object.keys(RVA) };
  },

  snapshot: function () { return readSession(); },
};
"""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gadget", default=GADGET)
    parser.add_argument("--out", default=DEFAULT_OUT)
    args = parser.parse_args()

    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if os.path.abspath(args.out).startswith(repo):
        print(f"REFUSING: {args.out} is inside the repo.", file=sys.stderr)
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
            "/home/banon/projects/er-effects-rs/scripts/frida-ersc-session-trace.py",
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
    counts = {"session": 0, "action": 0, "confirm": 0, "menu-open": 0}

    def on_message(message, _data):
        if message.get("type") != "send":
            print(f"[agent] {message}", file=sys.stderr)
            return
        p = message.get("payload") or {}
        handle.write(json.dumps(p) + "\n")
        handle.flush()
        kind = p.get("type")
        if kind in counts:
            counts[kind] += 1
        if kind == "osm":
            print(f"OSM captured {p['ptr']} via {p['via']}")
        elif kind == "menu-open":
            print(f"MENU OPEN group={p['group']}")
        elif kind == "confirm":
            print(f"CONFIRM selectedIndex={p['selectedIndex']}")
        elif kind == "action":
            print(f"ACTION {p['name']}")
        elif kind == "session":
            f = p["fields"]
            state = f["state"]
            name = STATE_NAMES.get(state, "UNKNOWN -- this is the interesting case")
            print(
                f"  session state=0x{state:02x} ({name}) "
                f"+0x1d4={f['f1d4']} latch=+0x1f0={f['latch']}  [{p['why']}]"
            )

    script.on("message", on_message)
    script.load()
    result = script.exports_sync.start()
    print(result)
    if result.get("error"):
        print(
            "REFUSING TO TRACE: the show() prologue did not match what static RE recorded, so "
            "these RVAs do not describe this ersc.dll. Hooking anyway would attach to the middle "
            "of unknown code.",
            file=sys.stderr,
        )
        handle.close()
        return 4

    print(f"tracing -> {args.out}")
    print("Use the Challenger's Lynchpin. Every menu open, confirm, action and session-state")
    print("transition is recorded, including whatever sets state 0x15.")

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
    print(
        f"menu-opens={counts['menu-open']} confirms={counts['confirm']} "
        f"actions={counts['action']} state-changes={counts['session']} -> {args.out}"
    )
    if counts["session"] == 0:
        print(
            "NO SESSION STATE WAS EVER READ. Either OSM was never captured (the menu was never "
            "opened and no action ran) or OSM+0x58 is not the session pointer on this build. "
            "Those need different fixes -- check whether an 'osm' record was emitted.",
            file=sys.stderr,
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
