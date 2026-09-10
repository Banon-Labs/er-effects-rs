// What does ersc's `show` actually hand over, and what did our DLL resolve instead?
//
// One question only: the menu object and session at the instant Seamless builds its option menu,
// printed beside the pointer `er_invasion_warp` picked by scanning. No writes, no replacements.
'use strict';

const ERSC_SHOW_RVA = 0x241a0;
const ERSC_INVADE_RVA = 0x25850;
const SESSION_AT_OWNER_OFFSET = 0x58;
const SESSION_STATE_OFFSET = 0x150;
const OPEN_DIALOG_RVA = 0xea0360;

const ersc = Process.findModuleByName('ersc.dll');
const game = Process.enumerateModules()[0];

function hex(p) { return p === null || p.isNull() ? 'null' : '0x' + p.toString(16); }
function follow(a) { try { return a.readPointer(); } catch (e) { return NULL; } }
function u32(a) { try { return a.readU32(); } catch (e) { return null; } }

send({ kind: 'modules', ersc: hex(ersc ? ersc.base : null), game: hex(game.base) });

if (ersc !== null) {
    Interceptor.attach(ersc.base.add(ERSC_SHOW_RVA), {
        onEnter: function (args) {
            const osm = args[0];
            const session = follow(osm.add(SESSION_AT_OWNER_OFFSET));
            send({
                kind: 'show',
                osm: hex(osm),
                session: hex(session),
                state: session.isNull() ? null : '0x' + (u32(session.add(SESSION_STATE_OFFSET)) >>> 0).toString(16),
                r12: '0x' + this.context.r12.toString(16)
            });
        }
    });
    Interceptor.attach(ersc.base.add(ERSC_INVADE_RVA), {
        onEnter: function (args) {
            const owner = args[0];
            const session = follow(owner.add(SESSION_AT_OWNER_OFFSET));
            send({
                kind: 'invade_called',
                owner: hex(owner),
                session: hex(session),
                state: session.isNull() ? null : '0x' + (u32(session.add(SESSION_STATE_OFFSET)) >>> 0).toString(16),
                from: hex(this.returnAddress)
            });
        }
    });
}

// Is r12 the menu object when the dialog opener is entered? Measure it, do not assume it.
Interceptor.attach(game.base.add(OPEN_DIALOG_RVA), {
    onEnter: function (args) {
        send({
            kind: 'open_dialog',
            dialog: hex(args[0]),
            r12: '0x' + this.context.r12.toString(16),
            rbx: '0x' + this.context.rbx.toString(16),
            rsi: '0x' + this.context.rsi.toString(16),
            rdi: '0x' + this.context.rdi.toString(16),
            r13: '0x' + this.context.r13.toString(16),
            r14: '0x' + this.context.r14.toString(16),
            r15: '0x' + this.context.r15.toString(16),
            ret: hex(this.returnAddress)
        });
    }
});

send({ kind: 'armed', note: 'use the Lynchpin; every pointer is reported, nothing is changed' });
