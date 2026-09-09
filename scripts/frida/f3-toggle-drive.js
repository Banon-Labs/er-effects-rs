// Drive the invasion-warp F3 toggle without taking the screen from the user.
//
// Two facts, both measured 2026-09-09 against the game's own key read:
//
//  1. A press injected from the compositor never reaches the game. The Wayland virtual keyboard,
//     X11 XTEST and uinput injectors each produced zero non-zero returns from the game's own
//     `GetAsyncKeyState`, and two of them were delivered to an unrelated application, because a
//     focus-addressed press goes wherever focus is.
//  2. Naming the game's own X window does not rescue it, and not because the targeting is wrong:
//     with Elden Ring unfocused the DLL stops polling the key at all. F3 polls fell from 767 in
//     two seconds to zero the moment focus moved, which is `drive.rs`'s `game_has_focus` gate
//     doing exactly what it was written to do -- an alt-tabbed F7 must not teleport the player.
//
// So a press that proves anything requires the game to be foreground, which means taking the
// user's screen. This drives both reads inside the process instead, scoped to the one DLL under
// test:
//
//   * `GetWindowThreadProcessId`, only when the caller's return address lies inside
//     `er_invasion_warp.dll`, reports this process -- so `game_has_focus()` answers true for that
//     DLL's poll and for nothing else in the game.
//   * `GetAsyncKeyState(VK_F3)` returns down + pressed-since exactly twice.
//
// Everything downstream is the real code: the edge latch, the rebind guard, `apply_enable_toggle`
// and the config write. Two presses rather than one, because a single press cannot tell a toggle
// from a latch stuck on -- the log must read OFF and then ON.
const VK_F3 = 0x72;

// down (0x8000) + pressed-since-last-call (0x0001), the two bits `MarkKeys::edge` reads.
const KEY_DOWN_AND_PRESSED = 0x8001;

// Both presses are scheduled on the game's OWN poll count, never on a timer. The first waits for
// the poller to settle after the rebind guard has swallowed its first-poll edge; the second waits
// for completed ticks after the first, which is what proves the first `apply_enable_toggle`
// returned rather than assuming a wall-clock gap covered it. A timer would be a sleep standing in
// for a readiness signal that is right here to be counted.
const FIRST_PRESS_AT_POLL = 50;

const TICKS_BETWEEN_PRESSES = 200;

const REPORT_EVERY_POLLS = 250;

const user32 = Process.findModuleByName('user32.dll');
const warp = Process.findModuleByName('er_invasion_warp.dll');

if (warp === null) {
    send({ tag: 'fatal', why: 'er_invasion_warp.dll is not loaded in this process' });
} else {
    const lo = warp.base;
    const hi = warp.base.add(warp.size);
    const ownPid = Process.id;

    let focusForced = 0;
    let polls = 0;
    let delivered = 0;
    let secondPressAt = 0;

    Interceptor.attach(user32.getExportByName('GetWindowThreadProcessId'), {
        onEnter(args) {
            this.out = args[1];
            this.fromWarp = this.returnAddress.compare(lo) >= 0 && this.returnAddress.compare(hi) < 0;
        },
        onLeave() {
            if (!this.fromWarp || this.out.isNull()) return;
            if (this.out.readU32() === ownPid) return;
            this.out.writeU32(ownPid);
            focusForced += 1;
        },
    });

    Interceptor.attach(user32.getExportByName('GetAsyncKeyState'), {
        onEnter(args) { this.vk = args[0].toInt32() & 0xffff; },
        onLeave(retval) {
            if (this.vk !== VK_F3) return;
            polls += 1;

            const first = delivered === 0 && polls >= FIRST_PRESS_AT_POLL;
            const second = delivered === 1 && polls >= secondPressAt;
            if (first || second) {
                delivered += 1;
                if (first) secondPressAt = polls + TICKS_BETWEEN_PRESSES;
                retval.replace(ptr(KEY_DOWN_AND_PRESSED));
                send({ tag: 'f3-delivered', nth: delivered, at_poll: polls });
                return;
            }

            if (polls % REPORT_EVERY_POLLS === 0) {
                send({ tag: 'poll', f3_polls: polls, focus_reports_forced: focusForced, f3_delivered: delivered });
            }
        },
    });

    send({ tag: 'armed', warp: warp.base.toString(), size: warp.size, pid: ownPid });
}
