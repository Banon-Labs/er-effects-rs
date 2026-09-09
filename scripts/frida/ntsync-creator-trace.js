// Name the code that creates Windows sync objects, and count them per caller module.
//
// # The measurement this exists to make
//
// A wedged boot on 2026-09-08 held 7,178 ntsync file descriptors out of 7,723 total, and the main
// thread sat in `WaitForSingleObject(fd 7725, INFINITE)` -- the single newest object in the whole
// process, with no other waiter and no owner. Two open issues meet at that number:
// `er-effects-rs-0xv6` (about 28 ntsync fds created per second and none closed) and
// `er-effects-rs-1742` (the boot deadlock itself). A leak that large means handles are created in
// a loop somewhere, and the deadlock is the process finally waiting on one nobody signals.
//
// `/proc` can count the descriptors but cannot say who made them. This can: every creation is
// attributed to the module its return address falls in, so "the game does this normally" and "one
// of our twenty-one shells does this every frame" become distinguishable.
//
// Hooked at the ntdll layer rather than kernel32, because that is the single choke point every
// higher-level wrapper funnels through.
'use strict';

const CREATORS = [
    'NtCreateEvent',
    'NtCreateSemaphore',
    'NtCreateMutant',
    'NtCreateKeyedEvent',
    'NtOpenEvent',
];
const CLOSERS = ['NtClose'];
// Report every this many creations, so a leak shows as a rate rather than a flood of lines.
const REPORT_EVERY = 200;

const counts = {};
let created = 0;
let closed = 0;

function moduleOf(address) {
    const m = Process.findModuleByAddress(address);
    if (m === null) {
        return 'anon:' + address;
    }
    return m.name + '+0x' + address.sub(m.base).toString(16);
}

function report() {
    send({ kind: 'ntsync_tally', created: created, closed: closed, by_caller: counts });
}

CREATORS.forEach(function (name) {
    const fn = Module.findExportByName('ntdll.dll', name);
    if (fn === null) {
        return;
    }
    Interceptor.attach(fn, {
        onEnter: function () {
            created += 1;
            const site = moduleOf(this.returnAddress);
            counts[site] = (counts[site] || 0) + 1;
            if (created % REPORT_EVERY === 0) {
                report();
            }
        }
    });
});

CLOSERS.forEach(function (name) {
    const fn = Module.findExportByName('ntdll.dll', name);
    if (fn !== null) {
        Interceptor.attach(fn, { onEnter: function () { closed += 1; } });
    }
});

send({ kind: 'armed', creators: CREATORS, closers: CLOSERS });
