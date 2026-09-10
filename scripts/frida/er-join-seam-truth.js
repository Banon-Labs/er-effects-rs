// Does the judge seam ever fire, and if not, what carries the join instead?
//
// The DLL arms `CS::SosSignMan::SetMultiplayJoinData` (1.16.2 0x1406fb520 -> 1.17.1 0x1406fc370)
// as the place it judges an incoming match. Run br-20260910-010746-7306 logged that hook armed
// once and then ZERO judgements -- no REJECT, no KEEP, no cancel -- while three `join-progress`
// lines show joins really happened. Either the seam is not on the join path at all, or it is and
// the DLL's detour is not reached.
//
// This settles which, and it writes nothing.
'use strict';

const SET_MULTIPLAY_JOIN_DATA = 0x6fc370;   // 1.17.1 runtime rva
const ERSC_INVADE = 0x25850;
const ERSC_CANCEL = 0x258d0;

const game = Process.enumerateModules()[0];
const ersc = Process.findModuleByName('ersc.dll');
let joinCalls = 0;

function where(address) {
    const m = Process.findModuleByAddress(address);
    return m === null ? '0x' + address.toString(16) : m.name + '+0x' + address.sub(m.base).toString(16);
}

Interceptor.attach(game.base.add(SET_MULTIPLAY_JOIN_DATA), {
    onEnter: function (args) {
        joinCalls += 1;
        send({
            kind: 'SetMultiplayJoinData',
            n: joinCalls,
            rcx: '0x' + args[0].toString(16),
            rdx: '0x' + args[1].toString(16),
            caller: where(this.returnAddress),
            thread: this.threadId
        });
    }
});

// NO HOOK ON ersc+0x25850 (invade) OR ersc+0x258d0 (cancel). This is not caution, it is a measured
// failure this probe caused on run br-20260910-011752-2910: er_invasion_warp byte-checks 64 bytes
// at the invade action before every call, an Interceptor trampoline sits on exactly those bytes,
// and the DLL then logs
//
//   ersc+0x25850 does not hold the 64 bytes this module measured for 2.0.1 -- refusing to call it
//
// and the item does nothing. `ersc-handoff.js` and `ersc-session-truth.js` both carry the same
// warning from 2026-09-09, where it cost 7,523 no-op re-arms. Watch the join seam in the GAME
// image instead; ersc's own entry points are off limits while the DLL is loaded.

send({
    kind: 'armed',
    seam: '0x' + game.base.add(SET_MULTIPLAY_JOIN_DATA).toString(16),
    note: 'invade; every SetMultiplayJoinData call is reported, and zero calls is itself the answer'
});
