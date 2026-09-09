// Find Seamless's option-menu object, hand it to er_invasion_warp.dll, and drive invade/cancel.
//
// # Why a scan and not a hook
//
// `ersc+0x25850` (invade) and `ersc+0x258d0` (cancel) both read `rcx` and nothing else, so calling
// either needs exactly one value: the option-menu object. A hook on invade hands it over, but only
// AFTER someone has already invaded -- which is useless for an agent that has to start the first
// one. The object is also reachable by reading, and er_invasion_warp does exactly that: walk
// ersc.dll's writable sections, and a qword whose `+0x58` points at something session-shaped is
// the menu object.
//
// Session shape, all measured against Seamless v2.0.1 and pinned in
// crates/er-invasion-warp/src/local_invasion_filter/ersc.rs:
//   +0x100  std::mutex   (_Type at +0x00, _Thread_id at +0x48, _Count at +0x4c)
//   +0x150  state        1 idle, 0x0e searching, 0x13 offer received, 0x23 cancelling
'use strict';

// The DLL's own detour on this address faults the game at 29.3s (three runs, ms_since_install
// 29288/29299/again, STATUS_ILLEGAL_INSTRUCTION at eldenring.exe+0x10043, mid-instruction inside
// the game's SSE block copy). A Frida Interceptor on the same address ran a whole session with no
// fault. So the observation happens here and the pointer is handed across the export instead.
const EXPORT_MODULE = 'er_invasion_warp.dll';
const EXPORT_NAME = 'er_invasion_warp_adopt_menu_object';
// The DLL starts the search on its own game task rather than us calling ersc from a Frida thread.
// Measured 2026-09-08: an RPC thread called ersc+0x25850 with a valid menu object and an idle
// session and never returned, because the action locks session+0x100 first; the game itself stayed
// healthy throughout, so it was that one call that parked.
const REQUEST_NAME = 'er_invasion_warp_request_invade';
let requestInvadeFn = null;
let adopt = null;
let adopted = null;

function handOver(osm) {
    if (adopted !== null && adopted.equals(osm)) {
        return;
    }
    if (adopt === null) {
        // Frida 17 removed the static `Module.findExportByName`; the lookup lives on the
        // module object now. Calling the old one throws `TypeError: not a function` from inside
        // the handoff, which is how the first drive of this agent died holding a valid pointer.
        const owner = Process.findModuleByName(EXPORT_MODULE);
        const address = owner === null ? null : owner.findExportByName(EXPORT_NAME);
        if (address === null) {
            send({ kind: 'export_missing', module: EXPORT_MODULE, name: EXPORT_NAME });
            adopt = null;
            return;
        }
        adopt = new NativeFunction(address, 'int', ['pointer']);
    }
    const accepted = adopt(osm);
    adopted = accepted ? osm : null;
    send({ kind: 'handed_over', osm: '0x' + osm.toString(16), accepted: accepted === 1 });
}

const NEXT_OBJECT = 0x58;
const STATE = 0x150;
const MUTEX = 0x100;
// Skip the confirm. Using an invasion item opens Seamless's menu and waits for a row to be
// confirmed; with this on, the search starts the moment the menu is built, so the item use goes
// straight to searching and the dialog is answered before it can be read.
//
// The call is made from inside the `show` hook deliberately: `show` runs on the game thread, and
// `ersc+0x25850` locks the session's std::mutex at +0x100 before it writes. Calling it from a
// Frida RPC thread parked there indefinitely on 2026-09-08 while the game itself stayed healthy at
// 687 CPU ticks per three seconds, so the thread the call is made on is the whole difference.
// The master switch for the skip. It stays ON across item uses -- a one-shot was tried on
// 2026-09-08 and was too blunt, since it stopped working after the first invasion of the session.
//
// What stops it fighting the player's own cancel is the idle check below, not a one-shot:
// pressing Cancel puts the session at 0x23 and it is not idle again until the search is genuinely
// over, so the menu Seamless rebuilds immediately after a cancel does not qualify.
let autoConfirm = true;
// The menu group `show` was called with when an auto-confirm last succeeded from an idle session.
// `null` until one has been seen.
//
// This is the gate that keeps the skip OUT of an active invasion. The invasion item does not do
// the same thing once you are in someone else's world -- it opens a different menu -- and driving
// the invade action from under that menu would be acting on a row the player did not choose. The
// group id is `show`'s second argument, so the menus are distinguishable at the seam; what is not
// yet known is which number belongs to which menu, and guessing it is exactly the kind of single
// observation promoted to a precondition that broke this feature on 2026-08-05.
//
// So: every menu is REPORTED with its group and the session state, the first auto-confirm from an
// idle session records its group, and every later one must match it. An unfamiliar group is left
// alone. Once the in-invasion menu's group is in the transcript it can be excluded by name.
let invadeMenuGroup = null;

const SHOW = 0x241a0;
const INVADE = 0x25850;
const CANCEL = 0x258d0;
const STATES = [0x01, 0x0e, 0x13, 0x23];
// MSVC's `_Mtx_internal_imp_t::_Type`: plain, try, or recursive. Anything else is not a mutex and
// the object is not a session, which is what keeps a look-alike qword out of the answer.
const MUTEX_TYPES = [0x01, 0x02, 0x100, 0x101, 0x102];

function readU32(pointer) {
    try {
        return pointer.readU32();
    } catch (error) {
        return null;
    }
}

function looksLikeSession(candidate) {
    if (candidate === null || candidate.isNull() || candidate.and(7).toInt32() !== 0) {
        return false;
    }
    const state = readU32(candidate.add(STATE));
    if (state === null || STATES.indexOf(state) < 0) {
        return false;
    }
    const type = readU32(candidate.add(MUTEX));
    return type !== null && MUTEX_TYPES.indexOf(type) >= 0;
}

// Cached because the walk crosses every writable byte of ersc.dll -- about 11 MB -- and doing that
// per call is what put the game's main thread at 100% of a core when the DLL's own version of this
// scan lost its cache.
let found = null;

function scan() {
    if (found !== null && looksLikeSession(found.session)) {
        return found;
    }
    const ersc = Process.findModuleByName('ersc.dll');
    if (ersc === null) {
        return null;
    }
    let examined = 0;
    for (const range of Process.enumerateRanges({ protection: 'rw-', coalesce: false })) {
        if (range.base.compare(ersc.base) < 0 || range.base.compare(ersc.base.add(ersc.size)) >= 0) {
            continue;
        }
        for (let offset = 0; offset + 8 <= range.size; offset += 8) {
            let holder;
            try {
                holder = range.base.add(offset).readPointer();
            } catch (error) {
                break;
            }
            examined += 1;
            if (holder.isNull() || holder.and(7).toInt32() !== 0) {
                continue;
            }
            let session;
            try {
                session = holder.add(NEXT_OBJECT).readPointer();
            } catch (error) {
                continue;
            }
            if (looksLikeSession(session)) {
                found = { slot: range.base.add(offset), osm: holder, session: session };
                send({
                    kind: 'osm_found',
                    slot: '0x' + found.slot.toString(16),
                    osm: '0x' + found.osm.toString(16),
                    session: '0x' + found.session.toString(16),
                    state: readU32(session.add(STATE)),
                    qwords_examined: examined
                });
                return found;
            }
        }
    }
    send({ kind: 'osm_not_found', qwords_examined: examined });
    return null;
}

function action(rva) {
    const ersc = Process.findModuleByName('ersc.dll');
    return new NativeFunction(ersc.base.add(rva), 'void', ['pointer']);
}

// Drive with an option-menu object the caller already knows, skipping the scan entirely.
//
// This is the path that actually works. `scan()` looks for a qword whose `+0x58` points at
// something session-shaped, and on 2026-09-08 that found nothing across all 11.5 MB of ersc.dll's
// writable data while er_invasion_warp's own scan resolved the pair from a different shape --
// `session 0x3d4383d8, owner 0x736046b8, via a pointer in ersc's own writable data at
// 0x1802f2680`. Rather than reimplement the DLL's scan a third time, take its answer.
//
// Passing an arbitrary pointer here is safe to the extent the disassembly says it is, and it says
// a lot: `ersc+0x25850` and `ersc+0x258d0` each touch `rcx` exactly once, in
// `mov rdi, [rcx+0x58]`, and never again. Everything after that is about the session.
function driveAt(rva, osmHex, label) {
    const osm = ptr(osmHex);
    let session;
    try {
        session = osm.add(NEXT_OBJECT).readPointer();
    } catch (error) {
        send({ kind: 'drive_refused', why: 'menu object +0x58 unreadable', osm: osmHex });
        return null;
    }
    // The state field alone, deliberately -- the same test er_invasion_warp's own
    // `adopt_menu_object` applies. Requiring a recognised `std::mutex` `_Type` at +0x100 as well
    // refused the real session on 2026-09-08: `stateAt` read 1 (idle) and the DLL accepted the
    // very same pointer, while this rejected it. The mutex value is reported rather than believed,
    // so a wrong guess about its shape can never again veto a correct pointer.
    const state = readU32(session.add(STATE));
    if (state === null || STATES.indexOf(state) < 0) {
        send({
            kind: 'drive_refused',
            why: 'the object at +0x58 carries no recognised session state',
            osm: osmHex,
            session: '0x' + session.toString(16),
            state: state,
            mutex_type: readU32(session.add(MUTEX))
        });
        return null;
    }
    const before = readU32(session.add(STATE));
    action(rva)(osm);
    const after = readU32(session.add(STATE));
    send({ kind: label, osm: osmHex, session: '0x' + session.toString(16),
           state_before: before, state_after: after });
    return { before: before, after: after };
}

// Find the object that OWNS a session, given the session's address.
//
// This is the half that breaks the chicken-and-egg. er_invasion_warp's own scan finds the session
// and logs it, but reports `owner 0x0` -- and both ersc actions take the OWNER as `rcx`, reading
// the session out of `[rcx+0x58]`, so a bare session cannot drive anything. Rather than wait for
// the player to invade once so the invade hook can hand `rcx` over, scan writable memory for a
// qword holding the session pointer: the address that holds it is `owner + 0x58`.
//
// Memory.scanSync with the pointer's own bytes does the search natively, which is why this can
// cross the whole address space rather than the 11.5 MB of ersc.dll that a JavaScript loop
// managed.
function ownerOf(sessionHex) {
    const session = ptr(sessionHex);
    let pattern = '';
    const raw = new Uint8Array(8);
    // Little-endian, byte by byte, because a NativePointer has no direct byte view. `.and(0xff)`
    // keeps each step inside pointer arithmetic rather than going through a 53-bit double.
    let value = session;
    for (let i = 0; i < 8; i += 1) {
        const byte = value.and(0xff).toInt32();
        raw[i] = byte;
        pattern += (i ? ' ' : '') + ('0' + byte.toString(16)).slice(-2);
        value = value.shr(8);
    }
    const candidates = [];
    for (const range of Process.enumerateRanges({ protection: 'rw-', coalesce: false })) {
        let hits;
        try {
            hits = Memory.scanSync(range.base, range.size, pattern);
        } catch (error) {
            continue;
        }
        for (const hit of hits) {
            if (hit.address.compare(NEXT_OBJECT) < 0) {
                continue;
            }
            const owner = hit.address.sub(NEXT_OBJECT);
            let back;
            try {
                back = owner.add(NEXT_OBJECT).readPointer();
            } catch (error) {
                continue;
            }
            if (!back.equals(session)) {
                continue;
            }
            candidates.push('0x' + owner.toString(16));
            if (candidates.length >= 16) {
                break;
            }
        }
        if (candidates.length >= 16) {
            break;
        }
    }
    send({ kind: 'owner_scan', session: sessionHex, candidates: candidates });
    return candidates;
}

rpc.exports = {
    // Turn the skip on or off without re-attaching, so it can be armed for one item use and left
    // off the rest of the time.
    setAutoConfirm: function (on) {
        autoConfirm = !!on;
        send({ kind: 'auto_confirm', enabled: autoConfirm });
        return autoConfirm;
    },
    ownerOf: ownerOf,
    // Hand every candidate to the DLL and keep the first it accepts. The DLL's own validation is
    // the arbiter -- it checks that `+0x58` leads to something carrying a live session state --
    // so a look-alike qword elsewhere in memory cannot become the pointer we drive with.
    // Reports candidates; adopts nothing. Same reason as `find`: the DLL cannot tell a real menu
    // object from a static that merely reads like one, so only the invade hook's `rcx` is allowed
    // to teach it.
    adoptOwnerOf: function (sessionHex) {
        return ownerOf(sessionHex);
    },
    // Ask the DLL to invade on its own game thread. This is the drive path that works; `invadeAt`
    // stays for the diagnostic case where the question is specifically what ersc does when called
    // from elsewhere.
    requestInvade: function () {
        if (requestInvadeFn === null) {
            const owner = Process.findModuleByName(EXPORT_MODULE);
            const address = owner === null ? null : owner.findExportByName(REQUEST_NAME);
            if (address === null) {
                send({ kind: 'export_missing', module: EXPORT_MODULE, name: REQUEST_NAME });
                return null;
            }
            requestInvadeFn = new NativeFunction(address, 'int', []);
        }
        const armed = requestInvadeFn();
        send({ kind: 'requested_invade', armed: armed === 1 });
        return armed;
    },
    invadeAt: function (osmHex) { return driveAt(INVADE, osmHex, 'drove_invade_at'); },
    cancelAt: function (osmHex) { return driveAt(CANCEL, osmHex, 'drove_cancel_at'); },
    stateAt: function (osmHex) {
        try {
            return readU32(ptr(osmHex).add(NEXT_OBJECT).readPointer().add(STATE));
        } catch (error) {
            return null;
        }
    },
    find: function () {
        // Reports only. The scan MUST NOT hand its answer to the DLL: on 2026-09-08 it produced
        // `0x1805c0740`, which is ersc.dll's own data section (base 0x180000000 + 0x5c0740), not a
        // heap object -- an ersc global whose `+0x58` happened to hold something with a plausible
        // state byte. The DLL accepted it, because its validation can only ask whether `+0x58`
        // leads to a readable session state, and a static that satisfies that is still not the
        // menu object. Driving ersc with it would call the invade action on a pointer nothing
        // owns.
        const hit = scan();
        return hit === null ? null : {
            osm: '0x' + hit.osm.toString(16),
            session: '0x' + hit.session.toString(16),
            state: readU32(hit.session.add(STATE))
        };
    },
    state: function () {
        const hit = scan();
        return hit === null ? null : readU32(hit.session.add(STATE));
    },
    // Both drive calls report the state either side of the call, because a Seamless action that
    // declines silently is indistinguishable from one that worked unless the field is read twice:
    // `ersc+0x25850` opens `cmp dword [rdi+0x150], 1` / `jne <return>`, so invading from any state
    // but idle simply returns.
    invade: function () {
        const hit = scan();
        if (hit === null) return null;
        const before = readU32(hit.session.add(STATE));
        action(INVADE)(hit.osm);
        const after = readU32(hit.session.add(STATE));
        send({ kind: 'drove_invade', state_before: before, state_after: after });
        return { before: before, after: after };
    },
    cancel: function () {
        const hit = scan();
        if (hit === null) return null;
        const before = readU32(hit.session.add(STATE));
        action(CANCEL)(hit.osm);
        const after = readU32(hit.session.add(STATE));
        send({ kind: 'drove_cancel', state_before: before, state_after: after });
        return { before: before, after: after };
    }
};

// Keep the DLL's pointer fresh from the one seam that is always right: whatever `rcx` the
// invade action was actually called with, whether that was the player's item or our own drive.
const erscBase = Process.findModuleByName('ersc.dll').base;

// `show(OSM, groupId)` -- Seamless building its option menu. Measured 2026-09-08: using the
// Challenger's Lynchpin OPENS this menu and stops there; the invade action does not run until the
// player confirms a row. So this is the seam that sees the object first, and on the item path it
// is the only one that runs at all until a confirm.
//
// This corrects the reasoning that put the DLL's own detour on the invade action: the 13-rejection
// run had zero menu-object captures not because `show` never ran, but because `ersc_observers` was
// false and the show observer was never installed.
Interceptor.attach(erscBase.add(SHOW), {
    // BEFORE `show` builds its rows, and that ordering is the feature rather than an accident.
    // Seamless picks the row list from the session state, so invading first means the menu that
    // opens already carries Cancel -- the player lands in a live search with a cancel one press
    // away, which is exactly what a search worth dropping needs.
    //
    // Driving it in `onLeave` instead was tried on 2026-09-08 and is wrong: `show` then builds the
    // IDLE list, so the open menu reads "Invade world as wanderer" while the session is already
    // searching, and pressing that row no-ops (`ersc+0x25850` opens `cmp dword [rdi+0x150], 1` /
    // `jne`). The player is left in a search they cannot cancel from the menu in front of them.
    //
    // The separate complaint that a search could not be dropped at all was the standing re-invade,
    // fixed by making this one-shot -- not by moving the call.
    onEnter: function (args) {
        handOver(args[0]);
        const group = args[1].toInt32();
        let session = null;
        let state = null;
        try {
            session = args[0].add(NEXT_OBJECT).readPointer();
            state = readU32(session.add(STATE));
        } catch (error) {
            session = null;
        }
        // Every menu, always. This is how the in-invasion menu's group gets identified rather than
        // guessed: open the item inside an invasion once and its number is in the transcript.
        send({ kind: 'menu_shown', group: group, state: state, learned_group: invadeMenuGroup });
        if (!autoConfirm || session === null) {
            return;
        }
        if (invadeMenuGroup !== null && group !== invadeMenuGroup) {
            send({ kind: 'auto_confirm_declined', group: group, why: 'not the invade menu group' });
            return;
        }
        // Only from idle, which is the same precondition the action itself enforces:
        // `ersc+0x25850` opens `cmp dword [rdi+0x150], 1` / `jne <return>`, so calling it from any
        // other state returns silently and would leave the menu looking armed while nothing ran.
        // It is also the first line of defence against firing inside an invasion, where the
        // session is not idle.
        const before = state;
        if (before !== 0x01) {
            return;
        }
        invadeMenuGroup = group;
        action(INVADE)(args[0]);
        send({
            kind: 'auto_confirmed',
            osm: '0x' + args[0].toString(16),
            state_before: before,
            state_after: readU32(session.add(STATE)),
            group: group
        });
    }
});

// NO HOOK ON THE CANCEL ACTION. Adding one here on 2026-09-08 broke the feature within minutes:
// er_invasion_warp's recurring build fingerprint reads `ersc+0x258d0`'s prologue on every use --
// it moved there precisely because it is the one entry point nothing patches -- and a Frida
// Interceptor overwrites those bytes. The DLL then read its own detour, concluded Seamless was a
// stranger, and logged `cannot cancel (WrongBlock) -- ErscUnrecognised` on a real rejection.
//
// Whatever needs observing about a cancel, observe it somewhere else.

Interceptor.attach(erscBase.add(INVADE), {
    onEnter: function (args) {
        handOver(args[0]);
    }
});

send({ kind: 'handoff_armed' });
