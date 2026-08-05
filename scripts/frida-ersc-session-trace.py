#!/usr/bin/env python3
"""Watch Seamless's session state machine and its option menu, live.

WHAT THIS ANSWERS
-----------------
The option table, the menu builder and the confirm hook are plaintext and fully read (2026-08-04).
Two things looked unreachable inside the Themida-virtualized `seamless_session_manager`:

  * what puts the session into state 0x15 -- the ONLY state in which "Seek opponent" appears; and
  * what consumes the opponent handle that "Seek opponent" latches.

THE FIRST ONE TURNED OUT NOT TO BE IN THE VM AT ALL (2026-08-05). An earlier pass concluded "nothing
plaintext sets 0x15" from an accessor search; that was a search artifact, not a fact. 0x15 is never
a literal ANYWHERE in the image -- it is COMPUTED, by five plain instructions at ersc+0x716e7, from
one bit. See S+0x00 below. The lesson generalises: "no immediate store of the value" is not the same
as "no plaintext writer", and a constant that is arithmetic on a flag will never appear as a literal.

The second is still unread, and is genuinely observable rather than readable: the latch is a plain
qword, so sampling it across a real session reconstructs that half from the outside.

WHAT IT WATCHES, and why each field
-----------------------------------
Everything hangs off ERSC's option-menu object, `OSM`, and its session object `S = OSM+0x58`:

  S+0x110  session state.  0 -> only "Invade world as a wanderer" is offered;
                           0x15 -> "Seek opponent" (index 0) + "Mark world" (index 1);
                           0x0D/0x0E/0x0F/0x11 -> only "Cancel search";
                           anything else -> the menu will not open at all.
           "Invade world as a wanderer" sets it to 0x0D; "Cancel search" sets 0x22 -- both as
           immediate stores. 0x15 is not stored, it is computed; see S+0x00.
  S+0x1D4  cleared to 0 by the "Seek opponent" action. One writer, zero plaintext readers.
  S+0x1F0  the CHOSEN OPPONENT'S HANDLE, latched by "Seek opponent". Zero plaintext consumers.
           This is the closest thing in the mod to "invade this specific person".
  S+0x00   THE FLAGS WORD THAT DECIDES 0x15. Found statically 2026-08-05, and it is NOT in the
           virtualized dispatcher after all -- the state is plain arithmetic on one bit, at
           ersc+0x716e7:
               mov rax,[rsp+0x30]      ; = *(u32*)S, loaded at ersc+0x71584
               shr eax,0x13            ; >> 19
               and eax,1               ; bit 19  (0x0008_0000)
               lea eax,[rax+rax*8]     ; x9
               add eax,0xc             ; +12
               mov [rdi+0x110],eax     ; 12 (0x0C) when clear, 21 (0x15) when SET
           So "Seek opponent" is offered exactly when bit 19 of S+0x00 is set. Watching that bit
           predicts the option's availability BEFORE the state byte changes, and distinguishes
           "the state machine never moved" from "the bit was never set".
  S+0x10C  update sentinel. `cmp dword [rdi+0x10c],0x7fffffff; je` at ersc+0x716db SKIPS the state
           write entirely. If state looks frozen while the flags bit flips, this is why -- so it
           is sampled rather than inferred.

WHAT IS PROVEN vs INFERRED: the derivation above is proven from the bytes. What bit 19 MEANS is
inferred from context (it sits right after a decrypt+validate of a 0x1C-byte buffer --
`xorps xmm0,[rbx]` then `call ersc+0x171f00`), which points at server-pushed state, consistent with
the destination arriving via CS::SosSignMan::SetMultiplayJoinData. Whether the bit can be
influenced from the client is UNTESTED, and this script does not try -- it only reads.

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

// Bit 19 of S+0x00 is the sole input to the 0x15 encoding (ersc+0x716ec: shr 0x13 / and 1).
const SEEK_FLAG_BIT = 0x00080000;
// `cmp dword [rdi+0x10c], 0x7fffffff; je` at ersc+0x716db skips the state write.
const STATE_UPDATE_SUPPRESSED = 0x7fffffff;

function readSession() {
  if (osm === null) return null;
  try {
    const S = osm.add(0x58).readPointer();
    if (S.isNull()) return null;
    const flags = S.readU32();
    const sentinel = S.add(0x10c).readU32();
    const state = S.add(0x110).readU32();
    // What the arithmetic at ersc+0x716e7 WOULD write given the flags right now. Recomputed here
    // rather than assumed, so a disagreement between predicted and actual is visible as data --
    // that gap is the signature of the 0x10c sentinel suppressing the write, or of some other
    // writer owning the field.
    const predicted = ((flags & SEEK_FLAG_BIT) ? 1 : 0) * 9 + 0xc;
    return {
      S: S.toString(),
      flags: '0x' + flags.toString(16),
      seekBit: (flags & SEEK_FLAG_BIT) !== 0,
      sentinel: '0x' + sentinel.toString(16),
      suppressed: sentinel === STATE_UPDATE_SUPPRESSED,
      state: state,
      predictedState: predicted,
      agrees: predicted === state,
      f1d4: S.add(0x1d4).readU8(),
      latch: S.add(0x1f0).readU64().toString(),
    };
  } catch (e) { return null; }
}

// Read-only snapshot of the region the "Seek opponent" action selects from.
//
// WHY A DUMP AND NOT A WALK: the action (ersc+0x24bc0) walks a list at OSM+0x60, cross-references a
// table reached via [OSM+0x10] -> * -> * -> +0x10ef0, and calls a vfunc (+0x48) on each entry. bd
// `accessor-FUN1402414a0-has-sideeffects-hang-is-notfound-alloc-safe-replicate-readonly-treewalk-only`
// records that calling that accessor has SIDE EFFECTS and can hang the game. So nothing is called
// here -- raw bytes only, decoded offline. Guessing the list layout and walking it live would risk
// the same hang for a structure we have not confirmed.
//
// Captured when state == 0x15, because that is the only state where the option is offered, and the
// user cannot realistically press it: if a candidate exists the invasion fires immediately, so the
// window only exists while the search is DRY.
let dumpedAt15 = false;
function dumpSeekCandidateRegion(S) {
  if (dumpedAt15) return null;
  function hex(ptr, len) {
    try {
      const b = ptr.readByteArray(len);
      if (b === null) return null;
      return Array.from(new Uint8Array(b))
        .map(function (x) { return ('0' + x.toString(16)).slice(-2); }).join('');
    } catch (e) { return null; }
  }
  const out = { osm: osm.toString(), S: S.toString() };
  out.osm_00_100 = hex(osm, 0x100);          // includes +0x10 (table root) and +0x60 (list)
  out.S_1c0_200  = hex(S.add(0x1c0), 0x40);  // includes +0x1d4 and the +0x1f0 latch
  // Follow the two pointers the action uses, one level, without calling anything.
  try {
    const listPtr = osm.add(0x60).readPointer();
    out.list_ptr = listPtr.toString();
    out.list_head = hex(listPtr, 0x80);
  } catch (e) { out.list_ptr = null; }
  // Follow the list's begin..end. The head at OSM+0x60 is a begin/end/cap triple (measured
  // 2026-08-05: begin=0x45cbcfe0 end=0x45cbd008 cap=0x45cbd010, a 0x28-byte span), so the ELEMENTS
  // are what the seek action iterates -- the head alone says only how many there are.
  try {
    const lp = osm.add(0x60).readPointer();
    const begin = lp.readPointer();
    const end = lp.add(8).readPointer();
    const span = end.sub(begin).toInt32();
    out.list_span = span;
    if (span > 0 && span <= 0x1000) {
      out.list_elems = hex(begin, span);
    }
  } catch (e) { out.list_span = null; }
  try {
    const t0 = osm.add(0x10).readPointer();
    out.t0 = t0.toString();
    const t1 = t0.readPointer();
    out.t1 = t1.toString();
    const t2 = t1.readPointer();
    out.t2 = t2.toString();
    out.table_10ef0 = hex(t2.add(0x10ef0), 0x40);   // count at +0, array ptr at +8
    // The array itself. Stride 0x10 per the static read of ersc+0x24bc0; 6 entries measured, so
    // cap generously and decode offline rather than trusting a stride guess here.
    const cnt = t2.add(0x10ef0).readU32();
    const arr = t2.add(0x10ef0 + 8).readPointer();
    out.table_count = cnt;
    out.table_arr = arr.toString();
    if (cnt > 0 && cnt <= 64) {
      out.table_entries = hex(arr, Math.min(cnt * 0x10, 0x400));
    }
  } catch (e) { out.t0 = out.t0 || null; }
  dumpedAt15 = true;
  return out;
}

function emitIfChanged(why) {
  const now = readSession();
  if (now === null) return;
  const key = JSON.stringify(now);
  if (key === last) return;
  last = key;
  send({ type: 'session', why: why, fields: now });
  if (now.state === 0x15) {
    try {
      const S = osm.add(0x58).readPointer();
      const dump = dumpSeekCandidateRegion(S);
      if (dump !== null) send({ type: 'seek-region', fields: dump });
    } catch (e) { /* unreadable */ }
  }
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

    // ---- THE JOIN OBSERVER (vanilla eldenring.exe, not ersc) ----
    //
    // CS::SosSignMan::SetMultiplayJoinData @0x1406FB520 is the single function that writes every
    // CSGameMan field the destination lives in, from a 128-byte server-pushed struct in RDX:
    //   SetTargetMapId(...)  -> GameMan+0xAC8   the BLOCK
    //   SetMultiplayJoinTargetBlockPos(...) -> +0xAA0   the position
    //   SetNPCInvadeTargetEntryPoint(0)     -> +0xAF0   hard zero, which is why it always read 0
    // Live CSGameMan sampling measured +0xAC8 changing BEFORE +0xAA0, so at THIS call the
    // destination is already decided and the player has not moved -- the exact window in which a
    // "is this the place I wanted?" filter could decide to stay or bail.
    //
    // Dumps the WHOLE struct rather than a guessed field offset. The field feeding SetTargetMapId
    // is named `matchPlayerCount` in the reversed signature, which is exactly the kind of name that
    // makes an offset guess wrong; decoding offline against GameMan+0xAC8 (sampled before AND
    // after the call) identifies it by CORRELATION instead of by trust.
    //
    // READ-ONLY. The call is not blocked and nothing is written -- this observes whether the filter
    // is possible, it does not implement one.
    const gameBase = Process.enumerateModules()[0].base;
    const joinAddr = gameBase.add(0x6fb520);
    const JOIN_PROLOGUE = '405348 81ec80000000'.replace(/ /g, '');
    let joinPro = '';
    try {
      joinPro = Array.from(new Uint8Array(joinAddr.readByteArray(9)))
        .map(function (x) { return ('0' + x.toString(16)).slice(-2); }).join('');
    } catch (e) { /* unreadable */ }
    if (joinPro === JOIN_PROLOGUE) {
      Interceptor.attach(joinAddr, {
        onEnter(args) {
          const rec = { type: 'join', data: null, gmBefore: null };
          try {
            const b = args[1].readByteArray(0x80);
            if (b !== null) {
              rec.data = Array.from(new Uint8Array(b))
                .map(function (x) { return ('0' + x.toString(16)).slice(-2); }).join('');
            }
          } catch (e) { /* unreadable */ }
          try {
            const gm = gameBase.add(0x3d69918).readPointer();
            rec.gmBefore = '0x' + gm.add(0xac8).readU32().toString(16);
            this.gm = gm;
          } catch (e) { /* no GameMan yet */ }
          send(rec);
        },
        onLeave() {
          try {
            if (this.gm) {
              send({ type: 'join-after', block: '0x' + this.gm.add(0xac8).readU32().toString(16) });
            }
          } catch (e) { /* unreadable */ }
        },
      });
      send({ type: 'join-hook', ok: true, addr: joinAddr.toString() });
    } else {
      send({ type: 'join-hook', ok: false, expected: JOIN_PROLOGUE, got: joinPro });
    }

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
    # Run-level verdict on the one question this trace exists to answer. Tracked here rather than
    # left to scrollback: "the bit never set" and "the bit set but the state never followed" need
    # completely different next steps, and both look like "Seek opponent never appeared" on screen.
    seen = {"seek_bit": False, "state_15": False, "disagreed": False, "suppressed": False}

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
        elif kind == "join-hook":
            print(
                f"JOIN OBSERVER {'armed at ' + p['addr'] if p['ok'] else 'REFUSED (prologue mismatch: got ' + str(p.get('got')) + ')'}"
            )
        elif kind == "join":
            print(f"JOIN DATA captured; GameMan+0xAC8 before = {p.get('gmBefore')}")
        elif kind == "join-after":
            print(f"JOIN DONE; GameMan+0xAC8 after = {p.get('block')}  <-- THE DESTINATION")
        elif kind == "seek-region":
            f = p["fields"]
            print(
                f"SEEK-REGION captured at state 0x15: OSM={f['osm']} S={f['S']} "
                f"list_ptr={f.get('list_ptr')} table={f.get('t2')}"
            )
        elif kind == "session":
            f = p["fields"]
            state = f["state"]
            name = STATE_NAMES.get(state, "UNKNOWN -- this is the interesting case")
            # Flags first: bit 19 is the CAUSE, the state byte is the effect.
            if f.get("seekBit"):
                seen["seek_bit"] = True
            if state == 0x15:
                seen["state_15"] = True
            if f.get("suppressed"):
                seen["suppressed"] = True
            if not f.get("agrees", True):
                seen["disagreed"] = True
            note = ""
            if not f.get("agrees", True):
                note = (
                    f"  <-- state disagrees with flags (predicted "
                    f"0x{f['predictedState']:02x})"
                    + (" -- 0x10c sentinel is suppressing the write" if f.get("suppressed") else "")
                )
            print(
                f"  flags={f['flags']} seek_bit={'SET' if f['seekBit'] else 'clear'} "
                f"sentinel={f['sentinel']}{' SUPPRESSED' if f.get('suppressed') else ''} "
                f"state=0x{state:02x} ({name}) "
                f"+0x1d4={f['f1d4']} latch=+0x1f0={f['latch']}  [{p['why']}]{note}"
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

    # The verdict, stated rather than left to be reconstructed from scrollback.
    print(
        f"seek bit (S+0x00 bit19) ever set: {seen['seek_bit']}; "
        f"state ever 0x15: {seen['state_15']}; "
        f"0x10c ever suppressing: {seen['suppressed']}; "
        f"flags/state ever disagreed: {seen['disagreed']}"
    )
    if not seen["seek_bit"]:
        print(
            "BIT 19 NEVER SET. 'Seek opponent' could not have been offered in this session -- the "
            "option is filtered on state 0x15 and 0x15 is that bit times nine plus twelve. This is "
            "upstream of anything in the menu: no amount of driving the UI reaches it. The next "
            "question is what sets the bit, and the decrypt+validate immediately upstream "
            "(xorps + ersc+0x171f00 over 0x1c bytes) says look at what the SERVER sent."
        )
    elif not seen["state_15"]:
        print(
            "BIT 19 WAS SET BUT THE STATE NEVER REACHED 0x15 -- the flags moved and the field did "
            "not follow. Check the 0x10c sentinel above; that branch skips the write outright.",
            file=sys.stderr,
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
