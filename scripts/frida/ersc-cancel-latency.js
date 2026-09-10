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

    send({ kind: 'armed', note: 'reject a world; request and completion are timestamped' });
}
