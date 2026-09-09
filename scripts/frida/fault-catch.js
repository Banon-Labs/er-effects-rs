// Catch the first-chance exception the moment it lands and say who caused it.
//
// # The question this exists to answer
//
// With `ersc_invade_observer = true` the game raises `STATUS_ILLEGAL_INSTRUCTION` at
// `eldenring.exe+0x10043`, twice, at `ms_since_install` 29288 and 29299 -- 11ms apart across two
// separate runs, one of them with no Frida in the process at all. The run with the observer off
// raised nothing. So the observer causes it and the timing is a schedule, not a race.
//
// 0x140010043 is MID-INSTRUCTION: decoding the game's own bytes from 0x140010000 gives a
// six-register SSE block copy whose `sub rdx, 6` sits at 0x140010041, four bytes long. Landing on
// +2 of it lands on `ea`, which has no 64-bit encoding -- hence the illegal instruction. Something
// transfers control INTO an instruction, and the crash logger's own backtrace is a fuzzy stack
// scan taken after the fact, which cannot say what.
//
// A Frida exception handler gets the real CPU context at the fault, before anything unwinds, and
// can walk every other thread in the same breath. It returns false so the game's own handling is
// unchanged: this observes, it does not swallow.
'use strict';

const DEPTH = 32;
let caught = 0;
// Enough to see whether the fault repeats with a different caller, few enough that a storm of
// them (22 access violations followed the first illegal instruction last time) cannot flood the
// transport and lose the first one, which is the only one that matters.
const MAX_REPORTS = 6;

function attribute(address) {
    if (address === null || address.isNull()) {
        return null;
    }
    const module = Process.findModuleByAddress(address);
    const hex = '0x' + address.toString(16);
    return module === null ? hex : hex + '{' + module.name + '+0x' + address.sub(module.base).toString(16) + '}';
}

// Hex rather than a raw buffer: `send`'s second argument must be an ArrayBuffer, and passing a
// plain object there throws `expected a buffer-like object` INSIDE the handler -- which loses the
// whole report and leaves only the throw in the log. That is what the first armed run produced,
// six times, so the bytes travel in the payload instead.
function bytesAround(address, before, after) {
    try {
        const start = address.sub(before);
        const raw = new Uint8Array(start.readByteArray(before + after));
        let hex = '';
        for (let i = 0; i < raw.length; i += 1) {
            hex += ('0' + raw[i].toString(16)).slice(-2);
        }
        return { from: attribute(start), before: before, hex: hex };
    } catch (error) {
        return { error: String(error) };
    }
}

Process.setExceptionHandler(function (details) {
    if (caught >= MAX_REPORTS) {
        return false;
    }
    caught += 1;
    const context = details.context;
    const report = {
        kind: 'fault',
        index: caught,
        type: details.type,
        address: attribute(details.address),
        memory: details.memory || null,
        thread_id: Process.getCurrentThreadId(),
        pc: attribute(context.pc),
        sp: '0x' + context.sp.toString(16),
        // Every register, because the corrupted value that produced the jump is in one of them and
        // guessing which costs a whole run.
        registers: {}
    };
    for (const name of ['rax', 'rbx', 'rcx', 'rdx', 'rsi', 'rdi', 'rbp',
                        'r8', 'r9', 'r10', 'r11', 'r12', 'r13', 'r14', 'r15']) {
        try {
            report.registers[name] = attribute(context[name]);
        } catch (error) {
            report.registers[name] = null;
        }
    }
    // The two backtracers disagree on this target and the disagreement is informative: ACCURATE
    // follows the frame pointer and says almost nothing on optimised game code, FUZZY scans the
    // stack and over-reports. The crash logger only ever had the fuzzy kind.
    for (const [key, mode] of [['accurate', Backtracer.ACCURATE], ['fuzzy', Backtracer.FUZZY]]) {
        try {
            report[key] = Thread.backtrace(context, mode).slice(0, DEPTH).map(attribute);
        } catch (error) {
            report[key] = [String(error)];
        }
    }
    // What every OTHER thread is doing at this instant. A fault that is really a deadlock symptom
    // looks completely different from one that is not, and that difference is only visible here.
    try {
        report.threads = Process.enumerateThreads().map(function (thread) {
            const row = { id: thread.id, state: thread.state };
            if (thread.context && thread.context.pc) {
                row.pc = attribute(thread.context.pc);
            }
            return row;
        });
    } catch (error) {
        report.threads = [String(error)];
    }
    report.bytes = bytesAround(context.pc, 0x20, 0x20);
    send(report);
    return false;
});

send({ kind: 'fault_catcher_armed', max_reports: MAX_REPORTS, depth: DEPTH });
