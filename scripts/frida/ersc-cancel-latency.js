// How long the 0x23 -> 0x24 wait really is, and what issues the request.
//
// Static reading (ersc.dll, Seamless v2.0.1) says the wait is a network completion, not a timer:
//   ersc+0x038a41  lea rax,[rip+0x5d598]     -> the std::function target is ersc+0x95fe0
//   ersc+0x038ab5  call qword ptr [rip+..]   -> issues the request (edx=8, r8d=0x40)
//   ersc+0x0960d6  cmp [r15+0x1d0], rsi      -> the completion must carry the matching handle
//   ersc+0x09610b  mov [r15+0x150], 0x24     -> the only 0x24 write in the module
// This measures request -> completion and names the call target from live memory, where it is a
// resolved pointer rather than an import slot.
'use strict';

const REQUEST_SITE_RVA = 0x038ab5;      // the indirect call
const CALL_SLOT_RVA = 0x1f65b8;         // where its target pointer lives
const COMPLETION_RVA = 0x95fe0;         // the callback that writes 0x24
const CANCEL_RVA = 0x258d0;             // ersc cancel: writes 0x23
const SESSION_AT_OWNER_OFFSET = 0x58;
const SESSION_STATE_OFFSET = 0x150;
const SESSION_HANDLE_OFFSET = 0x1d0;

const ersc = Process.findModuleByName('ersc.dll');
if (ersc === null) {
    send({ kind: 'error', why: 'ersc.dll not loaded' });
} else {
    const slot = ersc.base.add(CALL_SLOT_RVA);
    let target = NULL;
    try { target = slot.readPointer(); } catch (e) { /* not yet resolved */ }
    const owner = target.isNull() ? null : Process.findModuleByAddress(target);
    send({
        kind: 'call_target',
        slot: '0x' + slot.toString(16),
        target: target.isNull() ? 'null' : '0x' + target.toString(16),
        module: owner === null ? 'unresolved/anonymous' : owner.name,
        offset: owner === null ? null : '+0x' + target.sub(owner.base).toString(16),
        symbol: target.isNull() ? null : (DebugSymbol.fromAddress(target).name || 'no symbol')
    });

    let cancelAt = null;
    let requestAt = null;

    Interceptor.attach(ersc.base.add(CANCEL_RVA), {
        onEnter: function (args) {
            cancelAt = Date.now();
            const session = args[0].add(SESSION_AT_OWNER_OFFSET).readPointer();
            send({
                kind: 'cancel',
                t: cancelAt,
                session: '0x' + session.toString(16),
                state: session.add(SESSION_STATE_OFFSET).readU32(),
                handle: '0x' + session.add(SESSION_HANDLE_OFFSET).readPointer().toString(16)
            });
        }
    });

    Interceptor.attach(ersc.base.add(REQUEST_SITE_RVA), {
        onEnter: function () {
            requestAt = Date.now();
            send({
                kind: 'request_issued',
                t: requestAt,
                since_cancel_ms: cancelAt === null ? null : requestAt - cancelAt,
                rcx: '0x' + this.context.rcx.toString(16),
                edx: this.context.rdx.toNumber() & 0xffffffff,
                r8d: this.context.r8.toNumber() & 0xffffffff,
                target_now: '0x' + slot.readPointer().toString(16)
            });
        }
    });

    Interceptor.attach(ersc.base.add(COMPLETION_RVA), {
        onEnter: function (args) {
            const now = Date.now();
            let session = NULL, state = null, handle = 'unreadable';
            try {
                const self = args[0].add(8).readPointer();
                session = self.add(0xa0).readPointer();
                state = session.add(SESSION_STATE_OFFSET).readU32();
                handle = '0x' + session.add(SESSION_HANDLE_OFFSET).readPointer().toString(16);
                lastSession = session.toString();
            } catch (e) { /* report what we have */ }
            send({
                kind: 'completion',
                t: now,
                since_request_ms: requestAt === null ? null : now - requestAt,
                since_cancel_ms: cancelAt === null ? null : now - cancelAt,
                session: '0x' + session.toString(16),
                state_on_entry: state,
                handle: handle,
                arg_r8: '0x' + args[2].toString(16),
                // Static analysis found ZERO direct callers of this function, so who invokes it
                // only exists at runtime. This is the naming authority for that.
                caller: Thread.backtrace(this.context, Backtracer.FUZZY)
                    .slice(0, 6)
                    .map(function (a) {
                        const m = Process.findModuleByAddress(a);
                        return m === null
                            ? '0x' + a.toString(16)
                            : m.name + '+0x' + a.sub(m.base).toString(16);
                    })
            });
        }
    });

    // Name the target by walking the owning module's exports: it is a resolved pointer, not an
    // import slot, so DebugSymbol has nothing and the export table is the only naming authority.
    if (!target.isNull() && owner !== null) {
        let best = null;
        for (const e of owner.enumerateExports()) {
            if (e.type !== 'function') continue;
            const delta = target.sub(e.address).toInt32();
            if (delta >= 0 && (best === null || delta < best.delta)) {
                best = { name: e.name, delta: delta, at: e.address };
            }
        }
        send({
            kind: 'call_target_named',
            module: owner.name,
            export: best === null ? 'none at or below' : best.name,
            plus: best === null ? null : '+0x' + best.delta.toString(16),
            export_at: best === null ? null : '0x' + best.at.toString(16)
        });
    }

    // The event emitter the 0x24 writer runs from, and the only place the 30s can be seen.
    //
    // ersc+0x93be0(rcx, rdx) is a std::function fan-out: it loads a handler list and calls
    // [handler+0x10] with pointers to its own two arguments. The 0x24 writer is one of those
    // handlers. Its matching invocation came from ersc+0x28a85e -- inside the Themida-packed ERSC
    // section, which has no .pdata and no static readability, so what schedules it can only be
    // measured here.
    const EMITTER_RVA = 0x93be0;
    let cancelSeen = null;
    // Learned, not seeded. The previous version hardcoded 0x466ac930, which was correct for one
    // process and meaningless for the next: a session is heap-allocated per run. It is learned
    // from whichever seam speaks first -- cancel, the completion callback, or the DLL's own
    // adopted menu object read through ersc's show argument.
    let lastSession = '0';
    let window23 = null;
    let emissions = 0;
    Interceptor.attach(ersc.base.add(EMITTER_RVA), {
        onEnter: function (args) {
            emissions += 1;
            const now = Date.now();
            // Open the window on the SESSION STATE reading 0x23, not on our own cancel hook: a
            // rejection Seamless drives internally never touches ersc+0x258d0, and gating on that
            // hook reported zero events across a real reject.
            let state = null;
            try {
                state = ptr(lastSession).add(SESSION_STATE_OFFSET).readU32();
            } catch (e) { /* no session yet */ }
            if (state === 0x23) {
                if (window23 === null) {
                    window23 = now;
                }
            } else if (window23 !== null && now - window23 > 3000) {
                window23 = null;
            }
            if (window23 === null || now - window23 > 45000) {
                return;
            }
            const m = Process.findModuleByAddress(this.returnAddress);
            send({
                kind: 'emit',
                n: emissions,
                since_23_ms: now - window23,
                state: state,
                a: '0x' + args[0].toString(16),
                b: '0x' + args[1].toString(16),
                from: m === null
                    ? '0x' + this.returnAddress.toString(16)
                    : m.name + '+0x' + this.returnAddress.sub(m.base).toString(16),
                thread: this.threadId
            });
        }
    });
    // Mark t0 for the emitter window. The cancel hook above also reports; this is just the clock.
    Interceptor.attach(ersc.base.add(CANCEL_RVA), {
        onEnter: function (args) {
            cancelSeen = Date.now();
            try {
                lastSession = args[0].add(SESSION_AT_OWNER_OFFSET).readPointer().toString();
            } catch (e) { /* the cancel hook above already reports a bad owner */ }
        }
    });

    // ---------------------------------------------------------------------------------------
    // PHASE 2, disabled until phase 1 has named the matching emission. Flip to true and save.
    //
    // The skip, if the observation supports it: re-emit the event the 0x23 state machine is
    // waiting for, with the session's own handle, so the REAL callback runs its real body and
    // writes 0x24 itself. That is different in kind from storing 0x24 directly -- the callback
    // does whatever else it does, and nothing downstream is told a lie it can detect.
    //
    // It must run on a game thread: calling into ersc from a Frida JS thread parks forever on the
    // session mutex (measured 2026-09-08). So the re-emission piggybacks on the next emission we
    // observe, which is already on a thread ersc drove.
    // ---------------------------------------------------------------------------------------
    const SKIP_ENABLED = false;
    const emitter = new NativeFunction(ersc.base.add(EMITTER_RVA), 'void', ['pointer', 'pointer']);
    let skipFired = false;
    if (SKIP_ENABLED) {
        Interceptor.attach(ersc.base.add(EMITTER_RVA), {
            onLeave: function () {
                if (skipFired || cancelSeen === null) {
                    return;
                }
                let session = NULL, state = null, handle = NULL;
                try {
                    session = ptr(lastSession);
                    state = session.add(SESSION_STATE_OFFSET).readU32();
                    handle = session.add(SESSION_HANDLE_OFFSET).readPointer();
                } catch (e) { return; }
                if (state !== 0x23 || handle.isNull()) {
                    return;
                }
                skipFired = true;
                const before = Date.now();
                const box = Memory.alloc(16);
                box.writePointer(handle);
                emitter(box, box);
                send({
                    kind: 'skip_emitted',
                    since_cancel_ms: before - cancelSeen,
                    handle: '0x' + handle.toString(16),
                    state_after: session.add(SESSION_STATE_OFFSET).readU32()
                });
            }
        });
    }

    // `show(rcx = menu object)` is the earliest place the session is knowable: +0x58 leads to it.
    Interceptor.attach(ersc.base.add(0x241a0), {
        onEnter: function (args) {
            try {
                const session = args[0].add(SESSION_AT_OWNER_OFFSET).readPointer();
                if (!session.isNull() && lastSession === '0') {
                    lastSession = session.toString();
                    send({ kind: 'session_learned', from: 'show', session: '0x' + session.toString(16) });
                }
            } catch (e) { /* nothing to learn from this frame */ }
        }
    });

    send({
        kind: 'armed',
        note: 'reject a world; every emission for 45s after cancel is reported',
        skip_enabled: SKIP_ENABLED
    });
}
