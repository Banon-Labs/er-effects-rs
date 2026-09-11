// Is `top_window()` actually the pause menu's window on 1.17.1?
//
// The harness resolves it as CSMenuMan -> popupMenu (+0x80) -> currentTopMenuJob (+0xB0) ->
// window (+0x130), then scans the first 0x2000 bytes of that window for an embedded
// `CS::GridControl` pointer. On every run since 2026-09-05 the scan reports "no GridControl found
// in the top menu window" while a vtable scan of the whole heap finds six live instances, and
// `top_menu_id` (window+0x180) reads 0 or -1 where the 1.16.2 table says 0xffff or 0x25.
//
// Two offsets in that chain have already drifted between 1.16.2 and 1.17 -- window+0x180 and the
// OptionSetting tab chain at window+0x1870 -- so the suspicion is that +0x130 drifted too and
// `top_window()` returns a plausible heap pointer belonging to something else. This agent reads
// the chain live and answers it, rather than another rebuild-and-relaunch guessing at it.
//
// What it prints, once per invocation of `chain()`:
//   * each hop of the harness's chain, with the qword at every candidate offset near +0x130 that
//     looks like a heap pointer, so a drifted field shows up as a neighbour that does hold a
//     window;
//   * for each such candidate, whether scanning its first 0x2000 bytes finds a GridControl --
//     which is the exact question `pause_menu_grid` asks and currently answers no to.
//
// Run: python3 scripts/er-frida-up.py
//      uv run --with frida python3 scripts/er-frida-watch.py --agent scripts/frida/menu-window-chain.js

// The same floor game_mem.rs uses (0x10000), not a guess at where a heap starts. A first pass here
// used 0x10000000 and rejected every real pointer in this process -- the live GridControls sit at
// 0x3a80118 and 0x362a9ab8, three orders of magnitude below that -- so the chain read as null at
// its first hop and looked like a torn-down menu rather than a bad filter.
const HEAP_LO = ptr('0x10000');

function base() {
    const m = Process.findModuleByName('eldenring.exe');
    return m ? m.base : null;
}

function readPtr(addr) {
    try {
        const v = addr.readPointer();
        return v.compare(HEAP_LO) >= 0 ? v : null;
    } catch (_) {
        return null;
    }
}

function readI32(addr) {
    try {
        return addr.readS32();
    } catch (_) {
        return null;
    }
}

// `CS::GridControl`'s vtable, recovered from MSVC RTTI in eldenring-deobf-1.17.1.bin by
// scripts/er-rtti-map.py (`.?AVGridControl@CS@@` -> 0x142a94438). Identical in the 1.17.0 image,
// as expected: .rdata did not move in the 1.17.0 -> 1.17.1 shift, only .text did.
const GRID_CONTROL_VTABLE_RVA = 0x2a94438;
const GRID_SELECTED_CELL_OFFSET = 0xd4;
const WINDOW_SCAN_QWORDS = 0x400;

function gridInside(window, gridVtable) {
    for (let slot = 0; slot < WINDOW_SCAN_QWORDS; slot++) {
        const candidate = readPtr(window.add(slot * 8));
        if (candidate === null) {
            continue;
        }
        const vt = readPtr(candidate);
        if (vt !== null && vt.equals(gridVtable)) {
            return {
                offset: slot * 8,
                grid: candidate,
                cell: readI32(candidate.add(GRID_SELECTED_CELL_OFFSET)),
            };
        }
    }
    return null;
}

// The singleton the harness starts from, as a 1.17 RVA.
//
// `er-game-base`'s table is keyed by 1.16.2 (`CS_MENU_MAN_GLOBAL_RVA = 0x3d6b7b0`) and every .data
// global moved on 1.17, so the DLL translates at run time through `game_data_addr`. A Frida agent
// has no such resolver, so the carried value is written here directly:
// `scripts/map-data-rvas-1162-to-1170.py 0x3d6b7b0` answers 0x3d6f820, agreed by 846 references.
// Reading the untranslated 1.16.2 slot answers null, which reads as a torn-down menu rather than
// as the wrong address -- the failure this constant exists to avoid.
const CS_MENU_MAN_SINGLETON_RVA = 0x3d6f820;
const POPUP_MENU_OFFSET = 0x80;
const CURRENT_TOP_JOB_OFFSET = 0xb0;
const WINDOW_OFFSET = 0x130;
// How far into the job to look. 0x40 qwords (0x200 bytes) found nothing, and the declared offset
// turned out to hold UTF-16 text rather than a pointer, so the window is somewhere else entirely
// and the search has to be wider than the neighbourhood of a wrong answer.
const JOB_SCAN_QWORDS = 0x200;

function chain() {
    const b = base();
    if (b === null) {
        send('eldenring.exe module not found');
        return;
    }
    const gridVtable = b.add(GRID_CONTROL_VTABLE_RVA);
    const lines = [];

    const menuManSlot = b.add(CS_MENU_MAN_SINGLETON_RVA);
    const menuMan = readPtr(menuManSlot);
    lines.push(`CSMenuMan slot ${menuManSlot} -> ${menuMan}`);
    if (menuMan === null) {
        send(lines.join('\n'));
        return;
    }

    const popup = readPtr(menuMan.add(POPUP_MENU_OFFSET));
    lines.push(`  popupMenu (+0x80) -> ${popup}`);
    if (popup === null) {
        send(lines.join('\n'));
        return;
    }

    const job = readPtr(popup.add(CURRENT_TOP_JOB_OFFSET));
    lines.push(`  currentTopMenuJob (+0xb0) -> ${job}`);
    if (job === null) {
        send(lines.join('\n'));
        return;
    }

    // Every heap-pointer field in the job's first 0x200 bytes, not just +0x130. The one that owns
    // the pause menu's GridControl is the window, whatever its offset turns out to be.
    lines.push(`  scanning job for a window that contains a GridControl (declared offset 0x${WINDOW_OFFSET.toString(16)}):`);
    let found = 0;
    for (let slot = 0; slot < JOB_SCAN_QWORDS; slot++) {
        const offset = slot * 8;
        const candidate = readPtr(job.add(offset));
        if (candidate === null) {
            continue;
        }
        const hit = gridInside(candidate, gridVtable);
        const mark = offset === WINDOW_OFFSET ? '  <-- the offset the harness uses' : '';
        if (hit !== null) {
            found++;
            lines.push(
                `    job+0x${offset.toString(16)} -> ${candidate}  GridControl at +0x${hit.offset.toString(16)} ` +
                `(${hit.grid}) cell=${hit.cell}${mark}`
            );
        } else if (offset === WINDOW_OFFSET) {
            let text = '';
            try {
                text = ` utf16=${JSON.stringify(job.add(offset).readUtf16String(16))}`;
            } catch (_) {
                text = '';
            }
            lines.push(
                `    job+0x${offset.toString(16)} -> ${candidate}  no GridControl inside${text}${mark}`
            );
        }
    }
    lines.push(`  candidates containing a GridControl: ${found}`);
    send(lines.join('\n'));
}

// Dumped on load, and re-dumped by touching this file -- deliberately not on a timer.
//
// A `setInterval` poller is banned here (scripts/check-no-timeouts.py) and it would also be the
// wrong instrument: a timer samples whenever it happens to fire, so a reading taken between menus
// reports a null job and looks exactly like a drifted offset. `er-frida-watch.py` reloads this
// agent in place on every write, which is a real event under the operator's control, so the
// reading is always taken at a moment someone chose. Re-run it with:
//
//     touch scripts/frida/menu-window-chain.js
//
// `chain` is also on `rpc.exports` for a caller that drives the session programmatically.
rpc.exports = { chain };
chain();
