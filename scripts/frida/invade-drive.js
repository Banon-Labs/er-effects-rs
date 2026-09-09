// Find Seamless's option-menu object without waiting for the player, then drive invade and cancel.
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

const NEXT_OBJECT = 0x58;
const STATE = 0x150;
const MUTEX = 0x100;
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
    if (!looksLikeSession(session)) {
        send({
            kind: 'drive_refused',
            why: 'the object at +0x58 is not session-shaped',
            osm: osmHex,
            session: '0x' + session.toString(16)
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

rpc.exports = {
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

send({ kind: 'invade_driver_armed' });
