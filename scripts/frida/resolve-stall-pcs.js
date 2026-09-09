// Resolve a handful of stalled program counters to the module and offset that owns them.
//
// `Thread.backtrace` on this target returns raw addresses with no symbol, because the game and our
// own DLLs ship no exports for their internals. The module base plus offset is the usable form: an
// offset into `er_invasion_warp.dll` can be read straight out of our own disassembly, and an
// offset into `ntdll.dll` names the syscall a thread is parked on.
'use strict';

const WANTED = ['0x6ffff9e9778a', '0x6ffffff3ea94', '0x6ffffff409f4', '0x6ffff6b62b2c', '0x6fffff5650b4'];

const modules = Process.enumerateModules();
send({ kind: 'module_count', count: modules.length });

WANTED.forEach(function (text) {
    const address = ptr(text);
    let owner = null;
    for (let i = 0; i < modules.length; i++) {
        const m = modules[i];
        if (address.compare(m.base) >= 0 && address.compare(m.base.add(m.size)) < 0) {
            owner = { name: m.name, base: m.base.toString(), offset: address.sub(m.base).toString() };
            break;
        }
    }
    send({ kind: 'resolved', address: text, owner: owner });
});
send({ kind: 'done' });
