// Walk the live TitleTopDialog row vector and say what the two rows actually are.
//
// # Why this exists
//
// On 2026-09-09 a boot stalled at `PREPARING SAVE 6/11` forever. The DLL's own scan reported
// `row vector begin(0x1f8) end(0x200) stride=0x140 rows=2 sane=true` followed by
// `done hits=0 rows_walked=2 found_member_node=0x0 found_item=0x0` -- it walked the whole
// vector and recognised neither row, so it waits for a Load-Game node that never arrives.
//
// A log that says "I found nothing" does not say WHAT was there. This reads the same two rows
// out of the live process and reports every pointer they hold, attributed to a module where
// one owns it, so the rows can be named instead of guessed at.
//
// Read-only: no hook, no write, no allocation in the target.
'use strict';

// The addresses the DLL logged for this boot. They are heap pointers from one process and mean
// nothing in another, so each is validated before it is followed.
const OWNER = ptr('0xbe338000');
const DIALOG = ptr('0x12fc3880');

const ROWVEC_BEGIN = 0x1f8;
const ROWVEC_END = 0x200;
const REGISTRY = 0xa48;
const SOURCE = 0xa38;

function attribute(p) {
    if (p.isNull()) return null;
    const m = Process.findModuleByAddress(p);
    if (m === null) return { p: '0x' + p.toString(16), module: null };
    return { p: '0x' + p.toString(16), module: m.name + '+0x' + p.sub(m.base).toString(16) };
}

// `Memory.readByteArray` was removed in Frida 12; the NativePointer method is the live API.
// The old spelling throws a TypeError rather than faulting, so a catch-all around it reports
// every address in the process as unreadable -- which is exactly what it did on the first run.
function readable(p, size) {
    try { return p.readByteArray(size) !== null; } catch (e) { return false; }
}

// A row is 0x140 bytes; report every qword, and attribute the ones that are pointers into a
// module (a vtable) or into readable heap (a candidate object or string).
function dumpRow(base, index) {
    const row = { index: index, at: '0x' + base.toString(16), qwords: [] };
    for (let off = 0; off < 0x140; off += 8) {
        let q;
        try { q = base.add(off).readPointer(); } catch (e) { row.qwords.push({ off: off, err: 'unreadable' }); continue; }
        if (q.isNull()) continue;
        const a = attribute(q);
        const entry = { off: '0x' + off.toString(16), val: '0x' + q.toString(16) };
        if (a && a.module) entry.module = a.module;
        else if (readable(q, 16)) {
            entry.heap = true;
            // A menu row's label is a wide string; show it when the bytes read like one.
            try {
                const s = q.readUtf16String(64);
                if (s && s.length > 0 && /^[\x20-\x7e]+$/.test(s)) entry.utf16 = s;
            } catch (e) { /* not a string */ }
            try { entry.vtable = attribute(q.readPointer()); } catch (e) { /* no vtable */ }
        }
        row.qwords.push(entry);
    }
    return row;
}

function main() {
    const game = Process.findModuleByName('eldenring.exe');
    const out = {
        kind: 'title-row-walk',
        game_base: game ? '0x' + game.base.toString(16) : null,
        dialog_live: readable(DIALOG, 0x100),
        owner_live: readable(OWNER, 0x200)
    };
    if (!out.dialog_live) { send(out); return; }

    out.dialog_vtable = attribute(DIALOG.readPointer());
    out.registry = attribute(DIALOG.add(REGISTRY).readPointer());
    out.source = attribute(DIALOG.add(SOURCE).readPointer());

    const begin = DIALOG.add(ROWVEC_BEGIN).readPointer();
    const end = DIALOG.add(ROWVEC_END).readPointer();
    out.rowvec = { begin: '0x' + begin.toString(16), end: '0x' + end.toString(16) };
    if (begin.isNull() || end.isNull() || !readable(begin, 8)) { out.rowvec.usable = false; send(out); return; }

    const stride = 0x140;
    const span = end.sub(begin).toInt32();
    out.rowvec.stride = '0x140';
    out.rowvec.rows = Math.floor(span / stride);
    out.rows = [];
    for (let i = 0; i < out.rowvec.rows && i < 16; i++) out.rows.push(dumpRow(begin.add(i * stride), i));
    send(out);
}

main();
