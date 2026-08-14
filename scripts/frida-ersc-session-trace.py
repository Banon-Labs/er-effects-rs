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
let wanted = null;
// AUTO-CANCEL STATE.
// `lastActionArgs` holds the (arg0, arg1) the ENGINE passed on a real option press. The reject
// actuator replays those exact values rather than synthesising them: arg0 is OSM, but arg1 measured
// as 0x10f630 -- small enough to be a stack address, and fabricating a pointer for an argument whose
// use is unknown is how you crash someone's game. If no press has been observed this session the
// actuator REFUSES to fire; failing closed is the only safe default when the alternative is calling
// a function with invented arguments.
let lastActionArgs = null;
let autoCancel = false;
let cancelFn = null;
let cancelsFired = 0;
let cancelBudget = 0;      // 0 == unlimited
let matchArea = true;      // compare the AREA byte, not the whole block id
// RUNAWAY GUARD, expressed as CORRECTNESS rather than a count. A fixed budget cuts a WORKING loop
// short -- the user may legitimately need many rejects before the wanted area comes up. What must
// never happen is cancelling forever while the cancel is not actually working. So: after each
// auto-cancel the session is expected to return to idle (0x00). If several fire in a row and it
// never does, the actuator is not doing what it claims and disarms itself.
let cancelsSinceIdle = 0;
const CANCELS_WITHOUT_IDLE_LIMIT = 3;

// SELF-DRIVING RE-ARM.
// After an auto-cancel the session walks back to idle (0x00). Re-invading from there keeps the
// search alive without the user pressing anything, so a hunt for one area runs unattended.
//
// The re-invade MUST happen on the GAME THREAD. The state poll is a JS timer on frida's thread, and
// calling into ERSC from there would run session code concurrently with the game's own update. So
// the call is deferred to a hook on the session-update function (ersc+0x71420 -- the same function
// whose arithmetic computes state 0x15), which the engine calls continuously on its own thread.
//
// `inOurCall` separates OUR invocations from the user's. A cancel that arrives while it is false is
// the USER cancelling by hand, which is the agreed stop signal: self-driving disarms and the loop
// ends. Without that flag our own cancel would look like a user cancel and stop the loop instantly.
let pendingReinvade = false;
let inOurCall = false;
let invadeFn = null;
let reinvades = 0;
// Tick accounting. The re-invade fired from ersc+0x71420 never ran even though the session reached
// idle, and it produced no error -- meaning the CONDITION never held, not that the call failed. The
// prime suspect is the tick itself: 0x71420 is the SESSION UPDATE, so it plausibly stops being
// called once there is no session, i.e. exactly when the re-invade needs to happen. Counting ticks
// and recording how many arrive while idle turns that suspicion into a measurement.
let tickCount = 0;
let ticksWhilePending = 0;
let ticksWhileIdle = 0;

function noteIdleForCancelGuard(state) {
  if (state === 0x00) cancelsSinceIdle = 0;
}
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
      // FOLLOW EACH LIVE ENTRY. The whole question is whether a candidate carries its own map
      // location BEFORE the server commits a match -- if it does, a bad match can be filtered at
      // CANDIDATE time and never accepted, instead of accepted-then-cancelled. The table is already
      // populated during the search (measured: 1 live entry while state==0x15), which is strictly
      // earlier than SetMultiplayJoinData. Dump the pointee and look for a block id offline.
      out.entry_dumps = [];
      for (let i = 0; i < cnt && i < 8; i++) {
        try {
          const e = arr.add(i * 0x10).readPointer();
          if (e.isNull()) continue;
          out.entry_dumps.push({ i: i, ptr: e.toString(), bytes: hex(e, 0x200) });
        } catch (err) { /* unreadable entry */ }
      }
    }
  } catch (e) { out.t0 = out.t0 || null; }
  dumpedAt15 = true;
  return out;
}

// Re-invade from the POLL thread, and only at idle.
//
// This reverses my earlier caution, on measurement rather than instinct. The plan was to issue the
// re-invade from a game-thread hook on ersc+0x71420 (the session update). The tick diagnostic
// falsified that outright: `ticks=2 (while re-invade pending=1, while idle=0)` -- 0x71420 is a
// TRANSITION handler, not a per-frame update, and it never runs at idle. The re-invade was waiting
// on a clock that had already stopped.
//
// The same data licenses this path. ERSC session code is provably NOT executing while the session
// is idle (that is exactly what `whileIdle=0` means), so there is no concurrent session update to
// race. Restricting the call to state 0x00 keeps it in that quiescent window; it is never issued
// mid-search, mid-join, or during the 0x22 teardown.
function maybeReinvadeAtIdle(state) {
  if (!pendingReinvade || inOurCall || !autoCancel) return;
  if (state !== 0x00) return;
  if (osm === null || lastActionArgs === null) return;
  try {
    if (invadeFn === null) {
      invadeFn = new NativeFunction(base.add(RVA.invade), 'void',
                                    ['pointer', 'pointer', 'int', 'int']);
    }
    inOurCall = true;
    try {
      invadeFn(lastActionArgs[0], lastActionArgs[1], 1, 1);
    } finally { inOurCall = false; }
    pendingReinvade = false;
    reinvades++;
    send({ type: 'self-drive', state: 're-invaded', reinvades: reinvades, cancels: cancelsFired });
  } catch (e) {
    pendingReinvade = false;
    send({ type: 'self-drive', state: 'reinvade-failed', why: '' + e });
  }
}

function emitIfChanged(why) {
  const now = readSession();
  if (now === null) return;
  const key = JSON.stringify(now);
  if (key === last) return;
  last = key;
  noteIdleForCancelGuard(now.state);
  send({ type: 'session', why: why, fields: now });
  maybeReinvadeAtIdle(now.state);
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
  start: function (wantBlock, autoCancelOn, maxCancels, useAreaMatch) {
    wanted = (wantBlock === null || wantBlock === undefined) ? null : wantBlock;
    cancelBudget = maxCancels || 0;
    autoCancel = !!autoCancelOn;
    matchArea = !!useAreaMatch;
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
            // RECORD THE REAL ARGUMENTS. To automate a reject we must CALL cancel, and calling a
            // function whose signature we inferred from a decompile is how you crash someone's
            // game. Capturing what the engine actually passes on a genuine user-driven press turns
            // the ABI from a guess into a measurement -- replay these exact values instead.
            const argv = [];
            for (let a = 0; a < 4; a++) {
              try { argv.push(args[a].toString()); } catch (e) { argv.push('<?>'); }
            }
            try { lastActionArgs = [args[0], args[1]]; } catch (e) { /* keep prior */ }
            // A cancel we did NOT issue is the user stopping the hunt by hand -- the agreed exit.
            if (pair[0] === 'cancel' && !inOurCall && autoCancel) {
              autoCancel = false;
              pendingReinvade = false;
              send({ type: 'self-drive', state: 'disarmed-by-user',
                     cancels: cancelsFired, reinvades: reinvades });
            }
            send({ type: 'action', name: pair[0], args: argv, ret: this.returnAddress.toString() });
            emitIfChanged('action-enter:' + pair[0]);
          },
          onLeave() { emitIfChanged('action-leave:' + pair[0]); },
        });
      });

    // ---- GAME-THREAD TICK for the self-driving re-invade ----
    // ersc+0x71420 is the session update (it contains the bit-19 arithmetic that computes state
    // 0x15), so the engine calls it constantly on its own thread. Piggybacking gives a safe place to
    // issue the re-invade; doing it from the JS poll would call session code off-thread.
    Interceptor.attach(base.add(0x71420), {
      onEnter() {
        tickCount++;
        let state = -1;
        try { state = osm === null ? -1 : osm.add(0x58).readPointer().add(0x110).readU32(); }
        catch (e) { state = -1; }
        if (state === 0x00) ticksWhileIdle++;
        if (pendingReinvade) {
          ticksWhilePending++;
          if (ticksWhilePending === 1 || ticksWhilePending % 200 === 0) {
            send({ type: 'tick-diag', ticks: tickCount, whilePending: ticksWhilePending,
                   whileIdle: ticksWhileIdle, state: state });
          }
        }
        if (!pendingReinvade || inOurCall || !autoCancel) return;
        if (osm === null || lastActionArgs === null) return;
        if (state !== 0x00) return;   // only re-invade from a settled, idle session
        try {
          if (invadeFn === null) {
            invadeFn = new NativeFunction(base.add(RVA.invade), 'void',
                                          ['pointer', 'pointer', 'int', 'int']);
          }
          inOurCall = true;
          try {
            invadeFn(lastActionArgs[0], lastActionArgs[1], 1, 1);
          } finally { inOurCall = false; }
          pendingReinvade = false;
          reinvades++;
          send({ type: 'self-drive', state: 're-invaded', reinvades: reinvades,
                 cancels: cancelsFired });
        } catch (e) {
          pendingReinvade = false;
          send({ type: 'self-drive', state: 'reinvade-failed', why: '' + e });
        }
      },
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
          // The destination is at struct+0x00. Identified by CORRELATION against GameMan+0xAC8 on a
          // live invasion (0x0f000000, the only offset in 128 bytes that matched), not by trusting
          // the reversed field name -- which is `matchPlayerCount` and would have pointed elsewhere.
          try {
            const dest = args[1].readU32();
            rec.dest = '0x' + dest.toString(16);
            if (wanted !== null) {
              rec.wanted = '0x' + wanted.toString(16);
              const hit = matchArea
                ? ((dest >>> 24) === (wanted >>> 24))
                : (dest === wanted);
              rec.matchMode = matchArea ? 'area' : 'exact';
              rec.verdict = hit ? 'KEEP' : 'REJECT';
              this.verdict = rec.verdict;
            }
          } catch (e) { /* unreadable */ }
          send(rec);
        },
        onLeave() {
          try {
            if (this.gm) {
              send({ type: 'join-after', block: '0x' + this.gm.add(0xac8).readU32().toString(16) });
            }
          } catch (e) { /* unreadable */ }
          // THE REJECT. Fired from onLeave, not onEnter: the join data is fully written, so the
          // session is in the same shape it is when a human cancels, rather than half-updated.
          if (!(autoCancel && this.verdict === 'REJECT')) return;
          if (cancelBudget > 0 && cancelsFired >= cancelBudget) {
            send({ type: 'auto-cancel', ok: false, why: 'budget exhausted', fired: cancelsFired });
            return;
          }
          if (cancelsSinceIdle >= CANCELS_WITHOUT_IDLE_LIMIT) {
            send({
              type: 'auto-cancel', ok: false, fired: cancelsFired,
              why: 'DISARMED -- ' + cancelsSinceIdle + ' cancels fired without the session ever '
                   + 'returning to idle, so the cancel is not taking effect',
            });
            autoCancel = false;
            return;
          }
          if (lastActionArgs === null) {
            send({
              type: 'auto-cancel', ok: false,
              why: 'no real option press observed yet -- refusing to call cancel with invented args',
            });
            return;
          }
          try {
            if (cancelFn === null) {
              cancelFn = new NativeFunction(base.add(RVA.cancel), 'void',
                                            ['pointer', 'pointer', 'int', 'int']);
            }
            inOurCall = true;
            try {
              cancelFn(lastActionArgs[0], lastActionArgs[1], 1, 1);
            } finally { inOurCall = false; }
            cancelsFired++;
            cancelsSinceIdle++;
            pendingReinvade = true;
            send({ type: 'auto-cancel', ok: true, fired: cancelsFired,
                   args: [lastActionArgs[0].toString(), lastActionArgs[1].toString()] });
          } catch (e) {
            send({ type: 'auto-cancel', ok: false, why: '' + e });
          }
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
    parser.add_argument(
        "--auto-cancel",
        action="store_true",
        help=(
            "Arm the actuator: every match outside the wanted area is cancelled by calling "
            "ersc+0x24460 with the (arg0, arg1) captured from a REAL option press. Unlimited by "
            "default -- landing in the wanted place may legitimately take many rejects, and a fixed "
            "budget would cut a WORKING loop short. The runaway guard is a correctness condition "
            "instead: if cancels fire and the session stops returning to idle, the cancel is not "
            "taking effect and the actuator disarms itself. It also refuses to fire until a real "
            "option press has been observed, rather than calling with invented arguments."
        ),
    )
    parser.add_argument(
        "--max-cancels",
        type=int,
        default=0,
        help="Optional hard cap on auto-cancels. 0 (default) = unlimited.",
    )
    parser.add_argument(
        "--exact-block",
        action="store_true",
        help=(
            "Match the FULL block id instead of just the area byte. Default is area matching, so "
            "--want-block 0x0f000000 accepts any sub-block of that area -- 'somewhere in the "
            "Haligtree' rather than one exact tile."
        ),
    )
    parser.add_argument(
        "--want-block",
        default=None,
        help=(
            "Target block id, e.g. 0x0f000000. When set, every incoming match is judged against it "
            "at SetMultiplayJoinData -- the point where the destination is known and the player has "
            "not moved. THIS ONLY REPORTS. Cancelling is left to you, deliberately: whether bailing "
            "after the server pushed join data leaves the session clean is UNMEASURED, and that "
            "measurement should not be taken by a script guessing an ERSC action's ABI."
        ),
    )
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
            print(
                f"ACTION {p['name']}  args={p.get('args')}  called_from={p.get('ret')}"
            )
        elif kind == "tick-diag":
            print(
                f"TICK DIAG: session-update ticks={p['ticks']} (while re-invade pending="
                f"{p['whilePending']}, while idle={p['whileIdle']}) current_state=0x{p['state']:02x}"
            )
        elif kind == "self-drive":
            st = p.get("state")
            if st == "re-invaded":
                print(f"*** RE-INVADED automatically (#{p['reinvades']}) -- hunt continues ***")
            elif st == "disarmed-by-user":
                print(
                    f"*** YOU CANCELLED -- self-driving OFF after {p['cancels']} cancels / "
                    f"{p['reinvades']} re-invades ***"
                )
            else:
                print(f"SELF-DRIVE {st}: {p.get('why')}", file=sys.stderr)
        elif kind == "auto-cancel":
            if p.get("ok"):
                print(f"*** AUTO-CANCELLED (#{p['fired']}) -- rejected match dropped, search continues ***")
            else:
                print(f"AUTO-CANCEL DID NOT FIRE: {p.get('why')}", file=sys.stderr)
        elif kind == "join-hook":
            print(
                f"JOIN OBSERVER {'armed at ' + p['addr'] if p['ok'] else 'REFUSED (prologue mismatch: got ' + str(p.get('got')) + ')'}"
            )
        elif kind == "join":
            v = p.get("verdict")
            if v == "KEEP":
                print(f"*** MATCH KEEP: destination {p.get('dest')} IS your target -- let it load ***")
            elif v == "REJECT":
                print(
                    f"*** MATCH REJECT: destination {p.get('dest')} != target {p.get('wanted')} "
                    f"-- CANCEL NOW to re-roll ***"
                )
            else:
                print(f"JOIN DATA captured; destination = {p.get('dest')}")
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
    want = None
    if args.want_block is not None:
        want = int(args.want_block, 0)
        print(f"LOCATION FILTER ARMED: want block {want:#010x} -- mismatches will be reported, NOT cancelled")
    if args.auto_cancel and want is None:
        print("REFUSING: --auto-cancel without --want-block would cancel every match.", file=sys.stderr)
        return 5
    area_match = not args.exact_block
    if args.auto_cancel:
        scope = f"area {want >> 24:#04x}" if area_match else f"block {want:#010x}"
        cap = "unlimited" if args.max_cancels == 0 else f"max {args.max_cancels}"
        print(
            f"AUTO-CANCEL ARMED ({cap}): every match outside {scope} is cancelled automatically; "
            f"it disarms itself if cancels stop returning the session to idle."
        )
    result = script.exports_sync.start(want, args.auto_cancel, args.max_cancels, area_match)
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
