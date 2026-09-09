// Find Seamless's option-menu object by content, then start an invasion with it.
//
// The menu object is normally learned by watching the player press Invade, which means the mod can
// only ever react. Finding it directly makes the hunt startable by the agent, which AGENTS.md's
// 2026-07-22 standing order requires: the agent drives every input, never the player.
//
// The signature is the ASCII tag `seamless` at OSM+0x68 (measured live 2026-08-04) with a session
// pointer at +0x58. `scripts/ersc-osm-tagscan.py` was written for this and never ran against a
// session whose answer was known; here the answer validates itself, because calling
// `ersc+0x25850` on the wrong object does nothing while the right one moves the session from
// `0x01 IDLE` to `0x0e SEARCHING`. That transition is written by exactly one instruction in the
// action, so it cannot come from anywhere else.
'use strict';

const INVADE = 0x25850;
const NEXT_OBJECT = 0x58;
const TAG_OFFSET = 0x68;
const STATE = 0x150;
const TAG = 'seamless';

const ersc = Process.findModuleByName('ersc.dll');
if (ersc === null) {
    send({ kind: 'error', message: 'ersc.dll not loaded' });
} else {
    const invadeFn = new NativeFunction(ersc.base.add(INVADE), 'void', ['pointer']);
    const pattern = TAG.split('').map(function (c) {
        return c.charCodeAt(0).toString(16).padStart(2, '0');
    }).join(' ');

    const candidates = [];
    Process.enumerateRanges({ protection: 'rw-', coalesce: true }).forEach(function (range) {
        if (range.size > 64 * 1024 * 1024) {
            return;
        }
        let hits = [];
        try {
            hits = Memory.scanSync(range.base, range.size, pattern);
        } catch (error) {
            return;
        }
        hits.forEach(function (hit) {
            const osm = hit.address.sub(TAG_OFFSET);
            // The tag also appears inside longer strings and source paths, so alignment is the
            // first cheap filter on a text match.
            if (!osm.and(ptr(7)).isNull()) {
                return;
            }
            let session;
            let state;
            try {
                session = osm.add(NEXT_OBJECT).readPointer();
                state = session.add(STATE).readU32();
            } catch (error) {
                return;
            }
            if (state > 0x30) {
                return;
            }
            candidates.push({ osm: osm.toString(), session: session.toString(), state: state });
        });
    });

    send({ kind: 'osm_candidates', count: candidates.length, candidates: candidates.slice(0, 8) });

    // Drive only an unambiguous answer. Calling the action on the wrong object is the failure that
    // killed this process twice earlier in the session.
    const idle = candidates.filter(function (c) { return c.state === 0x01; });
    if (idle.length !== 1) {
        send({ kind: 'not_driving', reason: idle.length + ' idle candidate(s); need exactly one' });
    } else {
        const osm = ptr(idle[0].osm);
        const session = ptr(idle[0].session);
        const before = session.add(STATE).readU32();
        invadeFn(osm);
        const after = session.add(STATE).readU32();
        send({
            kind: 'agent_invade',
            osm: osm.toString(),
            session: session.toString(),
            state_before: before,
            state_after: after,
            worked: after === 0x0e
        });
    }
}
