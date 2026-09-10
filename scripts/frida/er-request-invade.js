// Ask er_invasion_warp.dll to start a Seamless search, with nothing hooked.
//
// # Why this can exist at all
//
// Driving ERSC's invade action needs a `this` whose `+0x58` is the session, and the crate makes
// one itself (`synthesized_owner`) rather than hunting for Seamless's -- both actions read `rcx`
// exactly once, at that offset, so whose box it is has no bearing. What the crate needs is a
// SESSION, and `er_invasion_warp_request_invade` arms its own game task to do the call on the
// thread that owns the session's `std::mutex`.
//
// Calling `ersc+0x25850` from a Frida RPC thread instead parks forever on that mutex -- measured
// 2026-09-08 with a valid menu object and an idle session, while the game itself stayed healthy.
// So this agent calls the crate's export and lets the game thread do the work.
//
// Nothing here attaches an Interceptor, which is the point: an Interceptor on ERSC's invade action
// overwrites the prologue the DLL byte-checks before every call, and the DLL then refuses to
// invade (7,523 no-op re-arms, measured 2026-09-09).
'use strict';

const MODULE = 'er_invasion_warp.dll';

function callExport(name, returnType, argTypes) {
    const owner = Process.findModuleByName(MODULE);
    if (owner === null) {
        send({ kind: 'module_missing', module: MODULE });
        return null;
    }
    const address = owner.findExportByName(name);
    if (address === null) {
        send({ kind: 'export_missing', module: MODULE, name: name });
        return null;
    }
    return new NativeFunction(address, returnType, argTypes)();
}

function invade() {
    const armed = callExport('er_invasion_warp_request_invade', 'int', []);
    send({ kind: 'requested_invade', armed: armed });
    return armed;
}

rpc.exports = { invade: invade };

// Fire on load, and on every hot-reload of this file.
//
// `er-frida-watch.py` has no way to call an rpc export -- it attaches, streams messages and
// reloads the file on edit -- so an agent that only exposed `rpc.exports` could never be triggered
// at all. Touching the file is the trigger instead, which suits the one thing this does: ask for a
// search, once, and report what the crate answered.
send({ kind: 'ready', module: MODULE });
invade();
