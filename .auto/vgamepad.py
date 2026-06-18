#!/usr/bin/env python3
"""Virtual Xbox-360 controller for the golden-reference capture.

Creates a uinput gamepad that Proton/ER reads via XInput. Crucially, gamepad input is POLLED by the
game regardless of window focus -- so it reaches ONLY the game, never the chat / other windows (the
mistake the keyboard approach made). The DLL does not hook XInput, so this passes straight through.

Usage:
    vgamepad.py create-and-listen   # create the pad, then read commands on stdin, one per line:
        A      -> tap the A/confirm button
        B      -> tap B/cancel
        UP/DOWN/LEFT/RIGHT -> tap the d-pad
        hold-ms <n> before a button to change the tap length (default 90ms)
        quit   -> destroy the pad and exit
The pad must exist BEFORE the game launches so SDL enumerates it.
"""
import os
import sys
import fcntl
import struct
import time

UINPUT_IOCTL_BASE = ord("U")


def _iow(nr, size):
    return (1 << 30) | (UINPUT_IOCTL_BASE << 8) | nr | (size << 16)


def _io(nr):
    return (UINPUT_IOCTL_BASE << 8) | nr


UI_SET_EVBIT = _iow(100, 4)
UI_SET_KEYBIT = _iow(101, 4)
UI_SET_ABSBIT = _iow(103, 4)
UI_DEV_CREATE = _io(1)
UI_DEV_DESTROY = _io(2)

EV_SYN, EV_KEY, EV_ABS = 0x00, 0x01, 0x03
SYN_REPORT = 0x00
BTN = {
    "A": 0x130, "B": 0x131, "X": 0x133, "Y": 0x134,
    "TL": 0x136, "TR": 0x137, "SELECT": 0x13A, "START": 0x13B,
    "THUMBL": 0x13D, "THUMBR": 0x13E,
}
DPAD = {"UP": 0x11, "DOWN": 0x11, "LEFT": 0x10, "RIGHT": 0x10}  # ABS_HAT0Y / ABS_HAT0X
DPAD_VAL = {"UP": -1, "DOWN": 1, "LEFT": -1, "RIGHT": 1}
ABS_X, ABS_Y, ABS_RX, ABS_RY, ABS_Z, ABS_RZ, ABS_HAT0X, ABS_HAT0Y = 0, 1, 3, 4, 2, 5, 0x10, 0x11
ABS_CNT = 64
UINPUT_MAX_NAME_SIZE = 80


def emit(fd, etype, code, value):
    os.write(fd, struct.pack("llHHi", 0, 0, etype, code, value))


def syn(fd):
    emit(fd, EV_SYN, SYN_REPORT, 0)


def create():
    fd = os.open("/dev/uinput", os.O_WRONLY | os.O_NONBLOCK)
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_KEY)
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_ABS)
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_SYN)
    for code in BTN.values():
        fcntl.ioctl(fd, UI_SET_KEYBIT, code)
    for axis in (ABS_X, ABS_Y, ABS_RX, ABS_RY, ABS_Z, ABS_RZ, ABS_HAT0X, ABS_HAT0Y):
        fcntl.ioctl(fd, UI_SET_ABSBIT, axis)
    name = b"Microsoft X-Box 360 pad".ljust(UINPUT_MAX_NAME_SIZE, b"\0")
    # input_id { bustype=BUS_USB(3), vendor=0x045e, product=0x028e, version=0x0114 }
    ident = struct.pack("HHHH", 3, 0x045E, 0x028E, 0x0114)
    ff_max = struct.pack("I", 0)
    absmax = [0] * ABS_CNT
    absmin = [0] * ABS_CNT
    for axis in (ABS_X, ABS_Y, ABS_RX, ABS_RY):
        absmax[axis] = 32767
        absmin[axis] = -32768
    for axis in (ABS_Z, ABS_RZ):
        absmax[axis] = 255
    for axis in (ABS_HAT0X, ABS_HAT0Y):
        absmax[axis] = 1
        absmin[axis] = -1
    zero = [0] * ABS_CNT
    dev = (
        name + ident + ff_max
        + struct.pack("%di" % ABS_CNT, *absmax)
        + struct.pack("%di" % ABS_CNT, *absmin)
        + struct.pack("%di" % ABS_CNT, *zero)
        + struct.pack("%di" % ABS_CNT, *zero)
    )
    os.write(fd, dev)
    fcntl.ioctl(fd, UI_DEV_CREATE)
    time.sleep(0.3)  # let udev/SDL settle
    return fd


def tap_button(fd, code, hold_s):
    emit(fd, EV_KEY, code, 1)
    syn(fd)
    time.sleep(hold_s)
    emit(fd, EV_KEY, code, 0)
    syn(fd)


def tap_dpad(fd, axis, val, hold_s):
    emit(fd, EV_ABS, axis, val)
    syn(fd)
    time.sleep(hold_s)
    emit(fd, EV_ABS, axis, 0)
    syn(fd)


def main():
    fd = create()
    print("vgamepad: created Microsoft X-Box 360 pad", flush=True)
    hold = 0.09
    try:
        for line in sys.stdin:
            tok = line.strip().split()
            if not tok:
                continue
            if tok[0] == "hold-ms":
                hold = int(tok[1]) / 1000.0
                continue
            if tok[0] == "quit":
                break
            cmd = tok[0].upper()
            if cmd in BTN:
                tap_button(fd, BTN[cmd], hold)
            elif cmd in DPAD:
                tap_dpad(fd, DPAD[cmd], DPAD_VAL[cmd], hold)
            print(f"vgamepad: tapped {cmd}", flush=True)
    finally:
        fcntl.ioctl(fd, UI_DEV_DESTROY)
        os.close(fd)


if __name__ == "__main__":
    main()
