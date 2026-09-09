// How many objects in the whole process share the REAL session's signature?
//
// The DLL's scan asks "state is one of four codes, and +0x100 looks like an MSVC mutex". Measured
// 2026-09-08 that matches 179,476 places at idle and 25,192 at an active state, so it identifies
// nothing. Frida then read the real session directly out of `ersc+0x25850` and it has a sharper
// shape than the DLL ever tested for: `_Type` is exactly 0x02 -- bare `_Mtx_try`, without the
// `_Mtx_plain` bit that `std::mutex` normally sets -- with `_Thread_id` 0xffffffff and `_Count` 0.
//
// This counts how many objects match that tighter signature. If the answer is one, the DLL can
// find the session on its own and needs no hook into ersc.dll at all.
'use strict';

const STATE = 0x150;
const MUTEX = 0x100;
const MTX_THREAD_ID = 0x48;
const MTX_COUNT = 0x4c;
const OBJECT_SPAN = 0x160;
// The session read by the invade hook this run, so the scan can report whether it found it.
const KNOWN_SESSION = ptr('0x469ac930');

let scanned = 0;
let loose = 0;
const tight = [];

Process.enumerateRanges({ protection: 'rw-', coalesce: true }).forEach(function (range) {
    if (range.size > 64 * 1024 * 1024) {
        return;
    }
    let bytes;
    try {
        bytes = range.base.readByteArray(range.size);
    } catch (error) {
        return;
    }
    const view = new DataView(bytes);
    for (let offset = 0; offset + OBJECT_SPAN <= range.size; offset += 8) {
        scanned += 1;
        const state = view.getUint32(offset + STATE, true);
        if (state !== 0x01) {
            continue;
        }
        const type = view.getUint32(offset + MUTEX, true);
        const count = view.getUint32(offset + MUTEX + MTX_COUNT, true);
        // What the DLL currently accepts.
        if ((type & 0x02) !== 0 && count <= 1) {
            loose += 1;
        }
        // What the real session actually reads.
        const owner = view.getUint32(offset + MUTEX + MTX_THREAD_ID, true);
        if (type === 0x02 && owner === 0xffffffff && count === 0) {
            tight.push(range.base.add(offset));
        }
    }
});

send({
    kind: 'signature_survey',
    qwords_scanned: scanned,
    loose_matches: loose,
    tight_matches: tight.length,
    tight_first: tight.slice(0, 12).map(function (p) { return p.toString(); }),
    known_session_found: tight.some(function (p) { return p.equals(KNOWN_SESSION); })
});
