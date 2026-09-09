// Cancel an invasion whose destination is not the block the player is standing in.
//
// # What this proves
//
// The DLL's own filter judges correctly and then cannot act, because it never finds Seamless's
// session object: a shape scan over the address space matches 179,476 places and the real session
// is in none of them (its mutex `_Type` reads `0x02`, which no candidate ever did). Frida does not
// have to search. `ersc+0x25850` opens `mov rdi, [rcx + 0x58]`, so the option-menu object arrives
// in `rcx` and the session is at `+0x58` -- handed over by the game at the moment the player
// invades, as a fact rather than a survivor.
//
// With that object in hand the enforcement is three steps, all of them the game's own code:
//
//   1. `CS::SosSignMan::SetMultiplayJoinData` fires once the server has chosen a destination and
//      before the player has moved. `ServerPushJoinData+0x00` is the destination block id --
//      identified by correlation in 2026-08-05 measurements, the only dword in 128 bytes that
//      matched what landed in `GameMan+0xAC8`.
//   2. `GetCurrentMapId` (game+0x5eefb0 on 1.16.2, +0x5efe00 on 1.17.1) writes the player's own
//      block through an out pointer.
//   3. If they differ, call `ersc+0x258d0` with the captured menu object. That is ERSC's own
//      Cancel row, so the session ends up in exactly the state a player's click would produce.
'use strict';

// Who enforces. `false` since 2026-09-08, when er_invasion_warp.dll gained its own read-only
// observer on the same `ersc+0x25850` and can therefore resolve the session by itself: with both
// of us armed the two would race to cancel the same match and neither log would describe what the
// DLL does on its own. Flip to `true` to put enforcement back here -- the cancel and re-invade
// paths below are unchanged and still proven; only the gate is new. Everything else keeps
// reporting either way, so this file stays the microscope while the DLL is under test.
const ENFORCE = false;

const INVADE = 0x25850;
const CANCEL = 0x258d0;
const NEXT_OBJECT = 0x58;
const STATE = 0x150;
// 1.17.1 relative virtual addresses, taken from the DLL's own translation lines and subtracted
// from the image base rather than eyeballed: the log prints
// `SetMultiplayJoinData: 0x1406fb520 -> 0x1406fc370` and `GetCurrentMapId: 0x1405eefb0 ->
// 0x1405efe00`, and the base is 0x140000000. Getting that subtraction wrong is silent -- an
// `Interceptor` on the wrong address installs cleanly and simply never fires, which is what a
// first attempt at this did with 0xafc370 while the player sat inside an invasion.
const JOIN_SEAM_RVA = 0x6fc370;
const GET_CURRENT_MAP_ID_RVA = 0x5efe00;
const JOIN_DATA_DESTINATION_BLOCK = 0x00;

let menuObject = null;
let cancelled = 0;
let seen = 0;
let reinvaded = 0;
// Set while our own re-invade is in flight, so the `invade` hook can tell the player's press from
// ours. Without it the log reads as though the player invaded twice and the hunt looks like theirs.
let drivingInvade = false;
// A re-invade is refused unless the session has settled back to idle, because that is what the
// action itself checks: `ersc+0x25850` opens `cmp dword [rdi + 0x150], 1` / `jne <return>`, so
// calling it from `0x23 CANCELLING` returns silently and the hunt would look armed while being
// dead. Poll for the settle instead of guessing a delay.
const SETTLE_POLL_MS = 200;
// Consecutive idle readings before the hunt is re-armed. Two, because the session passes through
// idle transiently and one reading would fire mid-transition; at 200ms that is a 400ms bail rather
// than the seconds a player spends reading Seamless's own failure message.
const IDLE_READINGS_TO_REARM = 2;
// A ceiling on automatic re-invades, so a session that can never match does not hunt forever.
const MAX_AUTOMATIC_REINVADES = 40;
let huntActive = false;
let idleReadings = 0;
let watchdogRunning = false;
// Set while our own cancel is in flight. Without it the player's own cancel -- using the invasion
// item to stop searching, which is the ordinary way out -- is indistinguishable from ours, and the
// watchdog would re-invade the instant they stopped. The mod would then be impossible to quit.
let drivingCancel = false;
// The stack that reached the invade, captured once from the player's own press.
let callerChain = null;

const ersc = Process.findModuleByName('ersc.dll');
const game = Process.findModuleByName('eldenring.exe');

if (ersc === null || game === null) {
    send({ kind: 'error', message: 'ersc.dll or eldenring.exe not loaded' });
} else {
    const cancelFn = new NativeFunction(ersc.base.add(CANCEL), 'void', ['pointer']);
    const getMapId = new NativeFunction(game.base.add(GET_CURRENT_MAP_ID_RVA), 'void', ['pointer']);

    // Learn the menu object from the player's own invade, which is the only place it is handed to
    // us. Nothing is driven here -- this call is the player's and is allowed to proceed.
    const invadeFn = new NativeFunction(ersc.base.add(INVADE), 'void', ['pointer']);

    Interceptor.attach(ersc.base.add(INVADE), {
        onEnter: function () {
            menuObject = this.context.rcx;
            // Capture the chain that reached the invade the first time the PLAYER drives it.
            //
            // The menu object is only ever handed over here, which makes the whole hunt dependent
            // on someone pressing the item -- and a scan for it fails, because the `seamless` tag
            // this repo's tooling looks for at OSM+0x68 is simply absent from v2.0.1 (measured
            // 2026-09-08: zero hits across every writable range). The caller chain is the way out:
            // whichever frame above this holds the object can be called directly, so one press
            // buys the ability to start every later hunt without one.
            if (!drivingInvade && callerChain === null) {
                try {
                    callerChain = Thread.backtrace(this.context, Backtracer.ACCURATE)
                        .slice(0, 16)
                        .map(function (address) {
                            const m = Process.findModuleByAddress(address);
                            return m === null
                                ? 'anon:' + address
                                : m.name + '+0x' + address.sub(m.base).toString(16);
                        });
                } catch (error) {
                    callerChain = ['<backtrace failed: ' + error.message + '>'];
                }
                send({ kind: 'invade_caller_chain', frames: callerChain });
            }
            if (!drivingInvade) {
                huntActive = true;
                startWatchdog();
            }
            idleReadings = 0;
            const session = menuObject.add(NEXT_OBJECT).readPointer();
            send({
                kind: 'invade',
                owner: menuObject.toString(),
                session: session.toString(),
                state: session.add(STATE).readU32(),
                driven_by_us: drivingInvade
            });
        }
    });

    // The player's own cancel ends the hunt. Anything else that returns the session to idle
    // re-arms it.
    //
    // Both halves are needed. Only re-invading after OUR cancel missed the case measured on
    // 2026-09-08 -- Seamless's own "Failed to invade. Could not invade host of session" ends the
    // attempt on its side, the session goes idle, and nothing restarts it, so a rejection we did
    // not cause costs the whole hunt. Watching the state instead of the cause covers that. But
    // watching state alone would also re-invade over the player's own item-cancel, which is how
    // they stop searching, so the cancel hook stands the hunt down when the call was not ours.
    Interceptor.attach(ersc.base.add(CANCEL), {
        onEnter: function () {
            if (drivingCancel) {
                return;
            }
            huntActive = false;
            idleReadings = 0;
            send({ kind: 'hunt_stopped', reason: 'the player cancelled the search themselves' });
        }
    });

    // Re-arm the hunt after a rejection, so a cancelled match costs the player a wait rather than
    // the whole attempt. The player pressed Invade once; every search after that is this mod
    // finishing the job they asked for.
    function startWatchdog() {
        if (watchdogRunning) {
            return;
        }
        watchdogRunning = true;
        tick();
    }

    function tick() {
        if (!huntActive || menuObject === null) {
            watchdogRunning = false;
            return;
        }
        const session = menuObject.add(NEXT_OBJECT).readPointer();
        const state = session.add(STATE).readU32();
        idleReadings = state === 0x01 ? idleReadings + 1 : 0;
        if (idleReadings >= IDLE_READINGS_TO_REARM) {
            idleReadings = 0;
            if (reinvaded >= MAX_AUTOMATIC_REINVADES) {
                huntActive = false;
                watchdogRunning = false;
                send({ kind: 'hunt_stopped', reason: 'reinvade ceiling reached', reinvaded: reinvaded });
                return;
            }
            drivingInvade = true;
            invadeFn(menuObject);
            drivingInvade = false;
            reinvaded += 1;
            send({
                kind: 'reinvaded',
                state_now: session.add(STATE).readU32(),
                reinvaded_total: reinvaded,
                cause: 'session returned to idle'
            });
        }
        setTimeout(tick, SETTLE_POLL_MS);
    }

    Interceptor.attach(game.base.add(JOIN_SEAM_RVA), {
        onEnter: function (args) {
            seen += 1;
            let destination = null;
            try {
                destination = args[1].add(JOIN_DATA_DESTINATION_BLOCK).readU32();
            } catch (error) {
                send({ kind: 'join_unreadable', error: error.message });
                return;
            }
            const out = Memory.alloc(4);
            out.writeU32(0xffffffff);
            getMapId(out);
            const anchor = out.readU32();
            // `GetCurrentMapId` writes 0xffffffff when the player has no block -- mid-load, mid-warp,
            // between areas. Comparing against that reads as "not local" and cancels EVERYTHING,
            // which is what happened live on 2026-09-08: eight consecutive cancels with
            // anchor=0xffffffff, each one re-arming the hunt, so the player could not invade at all.
            // An unresolved anchor is not evidence of a bad destination, so the match is left alone
            // -- the same fail-open the DLL takes with `anchor unresolved -- match left alone`.
            if (anchor === 0xffffffff) {
                send({
                    kind: 'anchor_unresolved',
                    destination: '0x' + destination.toString(16),
                    note: 'left alone rather than cancelled'
                });
                return;
            }
            const local = destination === anchor;
            send({
                kind: 'match',
                destination: '0x' + destination.toString(16),
                anchor: '0x' + anchor.toString(16),
                local: local,
                have_menu_object: menuObject !== null
            });
            if (local) {
                huntActive = false;
                send({ kind: 'kept', destination: '0x' + destination.toString(16) });
                return;
            }
            if (menuObject === null) {
                return;
            }
            const session = menuObject.add(NEXT_OBJECT).readPointer();
            const before = session.add(STATE).readU32();
            if (!ENFORCE) {
                // Report what would have happened and leave the match alone. The state is read
                // and named so a run says whether the DLL cancelled it -- `state_after` here is
                // whatever the DLL did, observed rather than caused.
                send({
                    kind: 'observed_not_cancelled',
                    destination: '0x' + destination.toString(16),
                    anchor: '0x' + anchor.toString(16),
                    state: before,
                    session: '0x' + session.toString(16),
                    menu_object: '0x' + menuObject.toString(16),
                    seen_total: seen,
                    note: 'ENFORCE is off -- er_invasion_warp.dll owns the cancel'
                });
                return;
            }
            drivingCancel = true;
            cancelFn(menuObject);
            drivingCancel = false;
            const after = session.add(STATE).readU32();
            cancelled += 1;
            send({
                kind: 'cancelled',
                destination: '0x' + destination.toString(16),
                anchor: '0x' + anchor.toString(16),
                state_before: before,
                state_after: after,
                cancelled_total: cancelled,
                seen_total: seen
            });
        }
    });

    // A reload resets `huntActive`, `menuObject` and `callerChain` to their initial values, which is
// deliberate: the running hunt does not survive an edit, so the next press is unambiguously the
// player's and its stack is the one worth capturing.
    send({
        kind: 'armed',
        ersc_base: ersc.base.toString(),
        game_base: game.base.toString(),
        join_seam: '+0x' + JOIN_SEAM_RVA.toString(16)
    });
}
