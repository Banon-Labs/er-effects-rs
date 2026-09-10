// Hand er_invasion_warp.dll the real Seamless menu object, and nothing else.
//
// # What this is for
//
// The crate has never once identified Seamless's session by itself. Every run logs
// `session resolved WITHOUT hooking Seamless ... owner 0x0` and then latches a look-alike -- five
// distinct fakes across five runs, each of which read `state_idle` straight through a join. The
// cancels that DID work on 2026-09-09 worked because `ersc-handoff.js` hooked the invade action
// and handed `rcx` across; that was mistaken for the crate working.
//
// So this agent exists to produce GROUND TRUTH: the address Seamless actually calls its actions
// with. Two uses, in order:
//
//   1. Immediately -- the DLL adopts it, so cancel and the rejection banner work this session.
//   2. Afterwards -- `session_scan::owner_among` is checked against it. If the crate's own owner
//      scan names the same pair, the scan is correct and Frida can be dropped; if it names a
//      different one, the scan is wrong and the log says so in one line instead of another
//      evening of invasions.
//
// # Both seams, because `show` alone does not see the path the player uses
//
// Seamless offers an invade two ways: from its own option menu, which calls `show`, and from the
// game's item-use path, which does not. Using an invasion item therefore never builds that menu,
// so a `show`-only hook captures nothing on the run that matters -- measured on
// `br-20260908-230004-d163`, where 13 matches were judged and rejected with zero menu captures in
// the whole log. `er_invasion_warp`'s own doc comment on its invade observer records the same
// thing. So the invade action is the primary seam here and `show` is the bonus.
//
// The invade hook carries a hazard `show` does not, and it is why both detach on first capture:
// the DLL byte-checks `ersc+0x25850`'s prologue before every call, an Interceptor trampoline sits
// on exactly those bytes, and a hook left attached makes the DLL refuse to invade -- measured
// 2026-09-09 as 7,523 no-op re-arms with `rearmed=0`. One capture is all that is needed, since the
// DLL keeps the adopted pointer for the life of the process.
//
// # What this deliberately does NOT do
//
// No auto-confirm, no driving of invade or cancel. `ersc-handoff.js` drives; this only observes
// and hands over, so the player's own presses are the only thing that starts a search. Attach it
// only once the world is loaded: attaching Interceptors to ersc.dll during boot reproduced the
// `eldenring.exe+0x10043` fault at +35.7s on 2026-09-09.
'use strict';

const SHOW = 0x241a0;
const INVADE = 0x25850;
const NEXT_OBJECT = 0x58;
const STATE = 0x150;
const EXPORT_MODULE = 'er_invasion_warp.dll';
const EXPORT_NAME = 'er_invasion_warp_adopt_menu_object';

let adopt = null;
let adopted = null;

function readU32(pointer) {
    try {
        return pointer.readU32();
    } catch (error) {
        return null;
    }
}

function handOver(osm) {
    if (adopted !== null && adopted.equals(osm)) {
        return true;
    }
    if (adopt === null) {
        const owner = Process.findModuleByName(EXPORT_MODULE);
        const address = owner === null ? null : owner.findExportByName(EXPORT_NAME);
        if (address === null) {
            send({ kind: 'export_missing', module: EXPORT_MODULE, name: EXPORT_NAME });
            return false;
        }
        adopt = new NativeFunction(address, 'int', ['pointer']);
    }
    const accepted = adopt(osm);
    let session = null;
    try {
        session = osm.add(NEXT_OBJECT).readPointer();
    } catch (error) {
        session = null;
    }
    send({
        kind: 'ground_truth',
        osm: '0x' + osm.toString(16),
        session: session === null ? null : '0x' + session.toString(16),
        state: session === null ? null : readU32(session.add(STATE)),
        accepted: accepted === 1
    });
    if (accepted === 1) {
        adopted = osm;
    }
    return accepted === 1;
}

const erscBase = Process.findModuleByName('ersc.dll').base;

// Both hooks detach the moment the DLL has the pointer, for the prologue reason above.
const hooks = {};

function watch(name, rva) {
    hooks[name] = Interceptor.attach(erscBase.add(rva), {
        onEnter: function (args) {
            const held = handOver(args[0]);
            // Detach on the pointer being HELD, not on this call having been the one to hand it
            // over: `handOver` returns early when the DLL already has this exact object, and a
            // re-attached agent on a process that adopted it earlier must still get out of the way.
            if (held) {
                for (const other of Object.keys(hooks)) {
                    if (hooks[other] !== null) {
                        hooks[other].detach();
                        hooks[other] = null;
                    }
                }
                send({ kind: 'hooks_detached', by: name });
            }
        }
    });
}

// The item path first, since that is the one the player uses.
watch('invade', INVADE);
watch('show', SHOW);

send({ kind: 'armed', ersc: '0x' + erscBase.toString(16), seams: ['invade', 'show'] });
