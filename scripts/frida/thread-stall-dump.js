// Dump every thread's state and top frames, to answer "what is the load waiting on".
//
// Hot-reloaded by `scripts/er-frida-watch.py`, so this can be edited while the game stays attached.
//
// A wchan histogram from Linux says "52 threads are parked", which is the normal idle state of a
// worker pool and tells you nothing. The Windows-side view says WHICH function each thread is
// parked in, and that is the difference between "the pool is asleep" and "the loader thread is
// blocked inside our own detour".
'use strict';

const MAX_FRAMES = 12;

function dump() {
    const threads = Process.enumerateThreads();
    send({ kind: 'thread_count', count: threads.length });
    threads.forEach(function (thread) {
        let frames = [];
        try {
            frames = Thread.backtrace(thread.context, Backtracer.FUZZY)
                .slice(0, MAX_FRAMES)
                .map(DebugSymbol.fromAddress)
                .map(function (symbol) { return symbol.toString(); });
        } catch (error) {
            frames = ['<no backtrace: ' + error.message + '>'];
        }
        send({
            kind: 'thread',
            id: thread.id,
            state: thread.state,
            pc: thread.context.pc ? thread.context.pc.toString() : null,
            pc_symbol: thread.context.pc ? DebugSymbol.fromAddress(thread.context.pc).toString() : null,
            frames: frames
        });
    });
    send({ kind: 'dump_complete' });
}

dump();
