// Walk every thread of a wedged Elden Ring and say where each one is stopped.
//
// # Why this exists
//
// On 2026-09-08 the game wedged during world load with `ersc_observers = true`, and the process
// was torn down before anything walked its threads. `/proc` said 130 threads, 77 in
// `ntsync_schedule` and 40 in `futex_wait` -- which proves a deadlock and names not one frame of
// it. The crash log's own backtrace covers a single thread, the one that faulted. A deadlock is a
// statement about two threads at once, so a per-thread walk is the only artifact that can answer
// it, and a torn-down process cannot produce one.
//
// Run it through `scripts/er-frida-stall-walk.py`, which attaches, loads this once, and writes
// the report. It changes nothing in the target: enumerate, read, report.
'use strict';

// How deep to walk. Deadlock chains here run through the game's task system into a mod DLL and
// back, and the interesting frame is rarely in the first handful.
const DEPTH = 48;

function attribute(address) {
    const module = Process.findModuleByAddress(address);
    if (module === null) {
        return { address: '0x' + address.toString(16), module: null };
    }
    return {
        address: '0x' + address.toString(16),
        module: module.name + '+0x' + address.sub(module.base).toString(16)
    };
}

function walk(thread) {
    // Both backtracers, because neither is reliable alone on this target. ACCURATE follows the
    // frame pointer and returns almost nothing on optimised game code that does not keep one;
    // FUZZY scans the stack for anything that looks like a return address and over-reports. The
    // union is what a reader actually wants, and disagreement between them is itself a signal.
    const out = { id: thread.id, state: thread.state, name: thread.name || null };
    if (thread.context && thread.context.pc) {
        out.pc = attribute(thread.context.pc);
        out.sp = '0x' + thread.context.sp.toString(16);
    }
    for (const [key, mode] of [['accurate', Backtracer.ACCURATE], ['fuzzy', Backtracer.FUZZY]]) {
        try {
            out[key] = Thread.backtrace(thread.context, mode)
                .slice(0, DEPTH)
                .map(attribute);
        } catch (error) {
            out[key] = [{ error: String(error) }];
        }
    }
    return out;
}

rpc.exports = {
    // Called by the runner rather than run at load, so the runner decides when the snapshot is
    // taken and can take more than one to see whether anything moved between them.
    walk: function () {
        const threads = Process.enumerateThreads();
        const modules = Process.enumerateModules().map(function (m) {
            return { name: m.name, base: '0x' + m.base.toString(16), size: m.size };
        });
        return {
            pid: Process.id,
            thread_count: threads.length,
            modules: modules,
            threads: threads.map(walk)
        };
    }
};
