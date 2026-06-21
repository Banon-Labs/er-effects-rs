#!/usr/bin/env python3
"""Static guard for native title-menu MenuWindowJob constructor provenance.

This intentionally checks a few small byte/table anchors in the repo-local decrypted
Elden Ring image. It does not prove runtime success; it prevents future agents from
forgetting which native path built the disabled Continue row and which constructor
families install native vs idle accept predicates.
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
IMAGE = ROOT / "eldenring-deobf.bin"

BASE = 0x1_4000_0000

IDLE_CTOR = 0x007ACF80
NATIVE_CTOR_A = 0x007AC8C0
NATIVE_CTOR_B = 0x007ACB00
TASK_ENQUEUE = 0x007A7B60

DISABLED_CONTINUE_CALL = 0x00764327
DISABLED_CONTINUE_ENQUEUE_CALL = 0x00764333
DISABLED_CONTINUE_ENQUEUE_RETURN = 0x00764338
DISABLED_NEIGHBOR_CALL = 0x00764457
DISABLED_NEIGHBOR_ENQUEUE_RETURN = 0x00764468
NATIVE_CTOR_A_TITLE_CALL = 0x009B6854
NATIVE_TITLE_READY_CALL = 0x009B676B
NATIVE_TITLE_READY_SKIP_JE = 0x009B6772
NATIVE_TITLE_REGISTER_CALL = 0x009B6880
NATIVE_ACCEPT_PREDICATE_LEA = 0x007AC962
IDLE_ACCEPT_PREDICATE_LEA = 0x007AD025
NATIVE_ACCEPT_PREDICATE = 0x007AD810
IDLE_ACCEPT_PREDICATE = 0x007ADD70
TITLE_DIALOG_READY_PREDICATE = 0x00733150
TITLE_MENU_REGISTER = 0x007A9250
LANG_SELECT_LABEL = 0x02B281D0
LANG_SELECT_COMPONENT_CTOR_CALL = 0x009B5A0A
LANG_SELECT_RESET_CALL = 0x009B5B04
LANG_SELECT_COMPONENT_CTOR = 0x0074A2F0
LANG_SELECT_SET_BOOL = 0x00733340
LANG_SELECT_READY_VTABLE = 0x02A94A70
LANG_SELECT_GETTER_SLOT0 = 0x0074BAF0
LANG_SELECT_GETTER_SLOT1 = 0x0074BAE0
LANG_SELECT_GETTER_BYTES = bytes.fromhex("488d4128c3")

CONTINUE_DOCALL = 0x00764B80
CONTINUE_DOCALL_IMPL = 0x00763FC0
CONTINUE_DOCALL_TABLE_SLOT = 0x02A9B9D8


def read_image() -> bytes:
    if not IMAGE.exists():
        raise AssertionError(f"missing decrypted image: {IMAGE}")
    return IMAGE.read_bytes()


def u64_at(data: bytes, rva: int) -> int:
    return struct.unpack_from("<Q", data, rva)[0]


def rel32_call_target(data: bytes, rva: int) -> int:
    if data[rva] != 0xE8:
        raise AssertionError(f"0x{BASE + rva:x} is not a rel32 call; byte=0x{data[rva]:02x}")
    imm = struct.unpack_from("<i", data, rva + 1)[0]
    return (rva + 5 + imm) & 0xFFFF_FFFF


def rel32_jcc_target(data: bytes, rva: int) -> int:
    if data[rva : rva + 2] != b"\x0f\x84":
        raise AssertionError(f"0x{BASE + rva:x} is not a rel32 je; bytes={data[rva:rva+2].hex()}")
    imm = struct.unpack_from("<i", data, rva + 2)[0]
    return (rva + 6 + imm) & 0xFFFF_FFFF


def rip_lea_target(data: bytes, rva: int) -> int:
    if data[rva : rva + 3] != b"\x48\x8d\x05":
        raise AssertionError(f"0x{BASE + rva:x} is not a RIP-relative lea into rax; bytes={data[rva:rva+3].hex()}")
    imm = struct.unpack_from("<i", data, rva + 3)[0]
    return (rva + 7 + imm) & 0xFFFF_FFFF


def find_rel32_callers(data: bytes, target_rva: int) -> list[int]:
    callers: list[int] = []
    i = 0
    limit = len(data) - 5
    while i < limit:
        i = data.find(b"\xE8", i)
        if i < 0:
            break
        imm = struct.unpack_from("<i", data, i + 1)[0]
        if ((i + 5 + imm) & 0xFFFF_FFFF) == target_rva:
            callers.append(i)
        i += 1
    return callers


def require(condition: bool, message: str, failures: list[str]) -> None:
    if not condition:
        failures.append(message)


def main() -> int:
    data = read_image()
    failures: list[str] = []

    require(
        rel32_call_target(data, DISABLED_CONTINUE_CALL) == IDLE_CTOR,
        "disabled Continue caller must call idle MenuWindowJob ctor 0x1407acf80",
        failures,
    )
    require(
        rel32_call_target(data, DISABLED_CONTINUE_ENQUEUE_CALL) == TASK_ENQUEUE,
        "disabled Continue caller must enqueue via exact 0x1407a7b60 after idle ctor",
        failures,
    )
    require(
        rel32_call_target(data, DISABLED_NEIGHBOR_CALL) == IDLE_CTOR,
        "neighbor disabled menu caller must share idle constructor family",
        failures,
    )
    require(
        rel32_call_target(data, NATIVE_CTOR_A_TITLE_CALL) == NATIVE_CTOR_A,
        "native-accept constructor A title caller anchor must call 0x1407ac8c0",
        failures,
    )
    require(
        rel32_call_target(data, NATIVE_TITLE_READY_CALL) == TITLE_DIALOG_READY_PREDICATE,
        "LangSelect native-accept builder is gated by 0x140733150(this+0x2610); this is diagnostic, not Continue proof",
        failures,
    )
    require(
        data[LANG_SELECT_LABEL : LANG_SELECT_LABEL + len(b"LangSelect\0")] == b"LangSelect\0",
        "title+0x2610 readiness descriptor must be LangSelect, not the Continue row",
        failures,
    )
    require(
        rel32_call_target(data, LANG_SELECT_COMPONENT_CTOR_CALL) == LANG_SELECT_COMPONENT_CTOR,
        "TosTitle ctor must construct title+0x2610 from the LangSelect descriptor through 0x14074a2f0",
        failures,
    )
    require(
        rel32_call_target(data, LANG_SELECT_RESET_CALL) == LANG_SELECT_SET_BOOL,
        "TosTitle ctor must reset LangSelect readiness through 0x140733340(..., false)",
        failures,
    )
    require(
        u64_at(data, LANG_SELECT_READY_VTABLE) == BASE + LANG_SELECT_GETTER_SLOT0
        and u64_at(data, LANG_SELECT_READY_VTABLE + 0x8) == BASE + LANG_SELECT_GETTER_SLOT1,
        "LangSelect component vtable must expose getter slots 0/1 used by readiness wrappers",
        failures,
    )
    require(
        data[LANG_SELECT_GETTER_SLOT0 : LANG_SELECT_GETTER_SLOT0 + len(LANG_SELECT_GETTER_BYTES)] == LANG_SELECT_GETTER_BYTES
        and data[LANG_SELECT_GETTER_SLOT1 : LANG_SELECT_GETTER_SLOT1 + len(LANG_SELECT_GETTER_BYTES)] == LANG_SELECT_GETTER_BYTES,
        "LangSelect readiness getter slots must return component+0x28, making flags live at component+0x48",
        failures,
    )
    require(
        rel32_jcc_target(data, NATIVE_TITLE_READY_SKIP_JE) == 0x009B6966,
        "native title builder ready failure must skip native-row construction",
        failures,
    )
    require(
        rel32_call_target(data, NATIVE_TITLE_REGISTER_CALL) == TITLE_MENU_REGISTER,
        "native title builder must register the native-accept row through 0x1407a9250(this+0x10, row)",
        failures,
    )
    require(
        rip_lea_target(data, NATIVE_ACCEPT_PREDICATE_LEA) == NATIVE_ACCEPT_PREDICATE,
        "native constructor A must install native accept predicate 0x1407ad810",
        failures,
    )
    require(
        rip_lea_target(data, IDLE_ACCEPT_PREDICATE_LEA) == IDLE_ACCEPT_PREDICATE,
        "idle constructor must install constant-false accept predicate 0x1407add70",
        failures,
    )
    require(
        data[CONTINUE_DOCALL : CONTINUE_DOCALL + 9]
        == bytes.fromhex("4883c108e937f4ffff"),
        "Continue docall must remain adjustor thunk to 0x140763fc0",
        failures,
    )
    require(
        u64_at(data, CONTINUE_DOCALL_TABLE_SLOT) == BASE + CONTINUE_DOCALL,
        "Continue docall table slot must point at 0x140764b80",
        failures,
    )

    idle_callers = find_rel32_callers(data, IDLE_CTOR)
    native_a_callers = find_rel32_callers(data, NATIVE_CTOR_A)
    native_b_callers = find_rel32_callers(data, NATIVE_CTOR_B)
    require(DISABLED_CONTINUE_CALL in idle_callers, "idle ctor xrefs must include disabled Continue callsite", failures)
    require(NATIVE_CTOR_A_TITLE_CALL in native_a_callers, "native ctor A xrefs must include title native-accept callsite", failures)
    require(len(native_b_callers) >= 1, "native ctor B must still have static callsites", failures)

    if failures:
        for failure in failures:
            print(f"FAIL {failure}", file=sys.stderr)
        return 1

    print(
        "menu constructor static checks passed: "
        f"idle_callers={len(idle_callers)} native_a_callers={len(native_a_callers)} "
        f"native_b_callers={len(native_b_callers)} "
        f"disabled_continue_return=0x{BASE + DISABLED_CONTINUE_ENQUEUE_RETURN:x} "
        f"disabled_neighbor_return=0x{BASE + DISABLED_NEIGHBOR_ENQUEUE_RETURN:x} "
        f"langselect_ready_skip=0x{BASE + rel32_jcc_target(data, NATIVE_TITLE_READY_SKIP_JE):x} "
        "langselect_flags_offset=component+0x48"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
