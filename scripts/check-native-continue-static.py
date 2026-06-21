#!/usr/bin/env python3
"""Static sanity checks for native Continue/menu edges used by zero-input autoload.

The repo-local eldenring-deobf.bin is gitignored and may be absent in CI. When absent this
check skips cleanly. When present, it validates small byte windows at exact RVAs so product
constants do not silently drift away from the reverse-engineered native ABI.
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
IMAGE = REPO_ROOT / "eldenring-deobf.bin"
IMAGE_BASE = 0x140000000

MENU_CONTINUE_WRAPPER = 0x14082BAC0
CONTINUE_LOAD = 0x14067B750
FD4_EVENT_CONSTRUCTOR = 0x1407A91E0
MENU_WINDOW_JOB_CTOR = 0x1407AC8C0
MENU_WINDOW_JOB_VTABLE = 0x142AA97E8
MENU_ACCEPT_IDLE = 0x1407ADD70
MENU_ACCEPT_NATIVE = 0x1407AD810
INPUT_MANAGER_READY = 0x140765F20
MENU_SUBMIT = 0x1407AC890


def fail(message: str) -> None:
    raise AssertionError(message)


def image_bytes(va: int, size: int) -> bytes:
    offset = va - IMAGE_BASE
    if offset < 0:
        fail(f"VA before image base: 0x{va:x}")
    with IMAGE.open("rb") as f:
        f.seek(offset)
        data = f.read(size)
    if len(data) != size:
        fail(f"short read at 0x{va:x}: wanted {size}, got {len(data)}")
    return data


def rel32_targets(va: int, data: bytes, opcode: int = 0xE8) -> set[int]:
    targets: set[int] = set()
    for index, byte in enumerate(data[:-4]):
        if byte != opcode:
            continue
        disp = struct.unpack_from("<i", data, index + 1)[0]
        targets.add(va + index + 5 + disp)
    return targets


def rip_lea_targets(va: int, data: bytes) -> set[int]:
    targets: set[int] = set()
    for index in range(0, len(data) - 6):
        # 48 8d /r disp32. We only need the common RIP-relative LEA forms used in these windows.
        if data[index : index + 2] != b"\x48\x8d":
            continue
        modrm = data[index + 2]
        if modrm & 0xC7 != 0x05:
            continue
        disp = struct.unpack_from("<i", data, index + 3)[0]
        targets.add(va + index + 7 + disp)
    return targets


def contains(data: bytes, needle: bytes, label: str) -> None:
    if needle not in data:
        fail(f"missing byte pattern for {label}: {needle.hex()}")


def main() -> int:
    if not IMAGE.exists():
        print(f"native Continue static check skipped: {IMAGE} is absent")
        return 0

    wrapper = image_bytes(MENU_CONTINUE_WRAPPER, 0x40)
    contains(wrapper, b"\x83\xc9\xff", "Continue wrapper passes slot=-1")
    contains(wrapper, b"\x33\xd2", "Continue wrapper passes flags=0")
    wrapper_calls = rel32_targets(MENU_CONTINUE_WRAPPER, wrapper)
    if CONTINUE_LOAD not in wrapper_calls:
        fail("Continue wrapper no longer calls continue_load_67b750")
    if FD4_EVENT_CONSTRUCTOR not in wrapper_calls:
        fail("Continue wrapper no longer posts an FD4 result event")

    ctor = image_bytes(MENU_WINDOW_JOB_CTOR, 0x220)
    if not ctor.startswith(b"\x40\x55\x56\x57\x41\x54\x41\x55\x41\x56\x41\x57"):
        fail("MenuWindowJob ctor prologue at 0x1407ac8c0 changed")
    ctor_leas = rip_lea_targets(MENU_WINDOW_JOB_CTOR, ctor)
    if MENU_WINDOW_JOB_VTABLE not in ctor_leas:
        fail("MenuWindowJob ctor no longer installs the expected vtable")
    if MENU_ACCEPT_NATIVE not in ctor_leas:
        fail("MenuWindowJob ctor no longer seeds the native accept predicate")

    idle = image_bytes(MENU_ACCEPT_IDLE, 0x3)
    if idle != b"\x33\xc0\xc3":
        fail("idle accept predicate is no longer the constant-false xor eax,eax; ret")

    native_accept = image_bytes(MENU_ACCEPT_NATIVE, 0x40)
    native_calls = rel32_targets(MENU_ACCEPT_NATIVE, native_accept)
    if INPUT_MANAGER_READY not in native_calls:
        fail("native accept predicate no longer calls input-manager readiness gate")
    contains(native_accept, b"\x0f\x94\xc1", "native accept returns !input_manager_ready")

    submit = image_bytes(MENU_SUBMIT, 0x40)
    submit_calls = rel32_targets(MENU_SUBMIT, submit)
    if FD4_EVENT_CONSTRUCTOR not in submit_calls:
        fail("native submit no longer constructs FD4 event code 3")
    contains(submit, b"\xff\x50\x60", "native submit dispatches result vtable +0x60")

    print("native Continue static checks passed")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AssertionError as exc:
        print(f"native Continue static check failed: {exc}", file=sys.stderr)
        raise SystemExit(1)
