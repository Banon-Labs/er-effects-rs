// Cancel an invasion the moment the session reaches SEARCHING, so the result is provable.
//
// A cancel fired while the session already reads `0x23 CANCELLING` writes 0x23 over 0x23 and moves
// nothing -- measured 2026-09-08, and indistinguishable from the call doing nothing at all. The
// only reading that proves the drive landed is `0x0e SEARCHING -> 0x23 CANCELLING`, because
// `ersc+0x258d0` is the sole writer of 0x23 into `session+0x150` and it takes the mutex first.
//
// # Why this hooks rather than polls
//
// This waited for `0x0e` by re-reading `session+0x150` on a `setTimeout` loop, which needed a menu
// object carried in from a previous run and a two-minute give-up. It does not have to: the sole
// writer of `0x0e` is `ersc+0x25850`, the invade action, and that function opens
// `mov rdi, [rcx + 0x58]` -- so the option-menu object arrives in `rcx` and the search starting is
// an event this agent can be attached to. Hooking it removes the poll, removes the stale address,
// and cancels from the game's own thread the way `ersc-session.js` already drives its cancel.
'use strict';

const INVADE = 0x25850;
const CANCEL = 0x258d0;
const NEXT_OBJECT = 0x58;
const STATE = 0x150;
const GUARD = 0x14c;
const SEARCHING = 0x0e;
const CANCELLING = 0x23;
// A menu object learned from the invade hook in an earlier session, kept for the one case the hook
// cannot cover: a search already in flight when this agent loads. It is a stale address in any new
// process, so the single read of it is guarded and a failure is reported rather than thrown.
const SEEDED_MENU_OBJECT = ptr('0x469ad518');

const ersc = Process.findModuleByName('ersc.dll');
if (ersc === null) {
    send({ kind: 'error', message: 'ersc.dll not loaded' });
} else {
    const cancelFn = new NativeFunction(ersc.base.add(CANCEL), 'void', ['pointer']);
    let done = false;

    function cancelIfSearching(menuObject, cause) {
        if (done) {
            return false;
        }
        let session;
        let state;
        try {
            session = menuObject.add(NEXT_OBJECT).readPointer();
            state = session.add(STATE).readU32();
        } catch (error) {
            send({ kind: 'unreadable', cause: cause, error: error.message });
            return false;
        }
        if (state !== SEARCHING) {
            send({ kind: 'not_searching', cause: cause, state_now: state });
            return false;
        }
        const guard = session.add(GUARD).readU32();
        cancelFn(menuObject);
        const after = session.add(STATE).readU32();
        done = true;
        send({
            kind: 'agent_cancel',
            cause: cause,
            menu_object: menuObject.toString(),
            session: session.toString(),
            guard: guard,
            state_before: SEARCHING,
            state_after: after,
            proved: after === CANCELLING
        });
        return true;
    }

    Interceptor.attach(ersc.base.add(INVADE), {
        onEnter: function (args) {
            // The object is `rcx` at entry. Saved here because the register context is gone by the
            // time `onLeave` runs.
            this.menuObject = args[0];
        },
        onLeave: function () {
            // The action has returned, so it has written `0x0e` into `session+0x150` and released
            // the mutex it took to do it. That write is the thing the old poll was looking for, and
            // this is the instant it has just happened.
            cancelIfSearching(this.menuObject, 'player_invaded');
        }
    });

    // One guarded look at the seeded object, for a search that started before this agent existed.
    cancelIfSearching(SEEDED_MENU_OBJECT, 'seeded_menu_object');
    send({ kind: 'armed', invade_hook: 'ersc+0x' + INVADE.toString(16) });
}
