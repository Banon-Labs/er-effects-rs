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
MENU_MEMBER_FUNC_JOB_RUN = 0x1409AABA0
MENU_REGISTRY_INSERT_COPY = 0x1407A7B60
RESULT_EVENT_HANDLER = 0x140746E80
RESULT_ACTION_BUILDER = 0x140746A00
MENU_JOB_SINGLE_CONSUMER = 0x1407A9600
MENU_JOB_LIST_CONSUMER = 0x1407AA1F0
MENU_OUT_IS_ACTIVE = 0x1407A9200
MENU_OUT_IS_ADVANCE = 0x1407A9210


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

    member_run = image_bytes(MENU_MEMBER_FUNC_JOB_RUN, 0x70)
    contains(member_run, b"\x4c\x8b\x41\x18", "MenuMemberFuncJob::run loads member function from node+0x18")
    contains(member_run, b"\x48\x63\x49\x20", "MenuMemberFuncJob::run sign-extends this-adjust from node+0x20")
    contains(member_run, b"\x48\x03\x48\x10", "MenuMemberFuncJob::run adds object pointer from node+0x10")
    contains(member_run, b"\x41\xff\xd0", "MenuMemberFuncJob::run calls the loaded member function")
    member_calls = rel32_targets(MENU_MEMBER_FUNC_JOB_RUN, member_run)
    if FD4_EVENT_CONSTRUCTOR not in member_calls:
        fail("MenuMemberFuncJob::run no longer posts FD4 event state after member call")

    registry_insert = image_bytes(MENU_REGISTRY_INSERT_COPY, 0x50)
    contains(registry_insert, b"\x48\x8b\x09", "registry insert loads source shared pointer [rcx]")
    contains(registry_insert, b"\x48\x89\x0a", "registry insert stores source shared pointer into [rdx]")
    contains(registry_insert, b"\x48\x83\xc1\x08", "registry insert retains copied shared pointer control block")

    result_handler = image_bytes(RESULT_EVENT_HANDLER, 0x100)
    contains(result_handler, b"\x80\xb9\xb0\x03\x00\x00\x00", "result handler gates action build on result+0x3b0")
    contains(result_handler, b"\xc6\x83\xb0\x03\x00\x00\x01", "result handler marks result+0x3b0 after building actions")
    result_calls = rel32_targets(RESULT_EVENT_HANDLER, result_handler)
    if RESULT_ACTION_BUILDER not in result_calls:
        fail("result handler no longer calls result action builder 0x140746a00")

    single_consumer = image_bytes(MENU_JOB_SINGLE_CONSUMER, 0x140)
    contains(single_consumer, b"\xff\x50\x10", "single native menu consumer calls job vtable +0x10 update")
    single_calls = rel32_targets(MENU_JOB_SINGLE_CONSUMER, single_consumer)
    if MENU_OUT_IS_ACTIVE not in single_calls:
        fail("single native menu consumer no longer classifies update out-state via 0x1407a9200")

    list_consumer = image_bytes(MENU_JOB_LIST_CONSUMER, 0x180)
    contains(list_consumer, b"\xff\x50\x10", "list native menu consumer calls job vtable +0x10 update")
    contains(list_consumer, b"\x48\x8b\x08\x48\x89\x0e", "list native menu consumer copies update return payload into caller out-state")
    contains(list_consumer, b"\xff\x47\x10", "list native menu consumer advances item cursor only after active/advance out-state")
    list_calls = rel32_targets(MENU_JOB_LIST_CONSUMER, list_consumer)
    if MENU_OUT_IS_ACTIVE not in list_calls or MENU_OUT_IS_ADVANCE not in list_calls:
        fail("list native menu consumer no longer validates update out-state via 0x1407a9200/0x1407a9210")

    print("native Continue static checks passed")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except AssertionError as exc:
        print(f"native Continue static check failed: {exc}", file=sys.stderr)
        raise SystemExit(1)
