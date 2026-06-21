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

# Disabled Continue row builder provenance (traced statically; NOT a product readiness
# predicate). The idle/constant-false-accept Continue row is built by the function at
# 0x140764290, which uses the Continue descriptor/vtable table at 0x142a9b808/958/9c8.
# Its SOLE caller is the title step-update method 0x140766980 (installed as a CSMenu
# step at 0x1407651cf with step vtable 0x142a9be20). The build is gated by two booleans
# on the title-step object: this+0x6b0 (build-request) and this+0x6b1 (suppress). The
# disabled row handle is stored at this+0x708. These offsets describe the DISABLED build
# path only; they are diagnostic provenance, not a Continue-armed/ready oracle.
DISABLED_CONTINUE_BUILDER = 0x00764290
DISABLED_CONTINUE_BUILDER_PROLOGUE = bytes.fromhex("40555657488d6c24b9")
DISABLED_CONTINUE_BUILDER_CALL = 0x00766E12
DISABLED_CONTINUE_GATE_6B0 = 0x00766DD1
DISABLED_CONTINUE_GATE_6B1 = 0x00766DDE
DISABLED_CONTINUE_GATE_6B0_BYTES = bytes.fromhex("80bbb006000000")
DISABLED_CONTINUE_GATE_6B1_BYTES = bytes.fromhex("80bbb106000000")
DISABLED_CONTINUE_STEP_UPDATE = 0x00766980
DISABLED_CONTINUE_STEP_UPDATE_INSTALL = 0x007651CF
DISABLED_CONTINUE_STEP_VTABLE_INSTALL = 0x007651C1
DISABLED_CONTINUE_STEP_VTABLE = 0x02A9BE20

# RTTI identities for the disabled-Continue builder chain (from
# docs/recon/deobf-rtti-classmap.tsv). They prove this region is CSMenuManImp's title
# MenuWindow-building update task and that the row's accept is a constant-false (idle)
# std::function lambda -- i.e. the idle row the product must NOT promote, and which is
# unrelated to title+0x2610 LangSelect readiness.
#   0x142a9be20 = CSEzUpdateTask<CSEzTask, CSMenuManImp>  (the step; owner this = CSMenuManImp menu obj)
#   0x142a9b958 = _Func_base<MenuWindow*, SceneProxy&>    (Continue row docall functor base)
#   0x142a9b9c8 = _Func_impl<lambda, MenuWindow*, SceneProxy&> (the lambda impl)
#   0x142a9bcb8 = NullPlayerMenuCtrl  (installed at this+0x6a8 -- title has no player)
#   0x142a9be00 = BackScreenData      (installed at this+0x710)
CONTINUE_FUNC_BASE_LEA = 0x007642DE
CONTINUE_FUNC_BASE_VTABLE = 0x02A9B958
CONTINUE_FUNC_IMPL_LEA = 0x007642E9
CONTINUE_FUNC_IMPL_VTABLE = 0x02A9B9C8
NULL_PLAYER_MENU_CTRL_INSTALL = 0x00765127
NULL_PLAYER_MENU_CTRL_VTABLE = 0x02A9BCB8
BACKSCREEN_DATA_INSTALL = 0x00765152
BACKSCREEN_DATA_VTABLE = 0x02A9BE00

# REAL native Load-Game job (the zero-input autoload target). A menu Continue/Load action
# enqueues a std::function LoadJob of type MenuJobResult(LoadJobContext&). The callback at
# 0x14082c240 allocates a 0x280000-byte buffer (== ER save-slot size) via vtable [rax+0x50]
# and reads/scans the slot -- this is the actual save read, reached through the MenuJob/
# LoadJob factory (functor vtable 0x142ac7728 built at 6 sites in 0x140827xxx-0x14082axxx),
# NOT the CSMenuManImp idle/disabled Continue row above. measure.sh's required runtime
# trace hits (0x14082c240/0x14082c2c8/0x14082c374/0x14067a810/0x14082c521) all live here.
# Our own stepper must drive THIS job zero-input; pin its identity so the path is not lost.
#   0x142ac7188 = _Func_base<MenuJobResult, LoadJobContext&>
#   0x142ac7728 = _Func_impl<MenuJobResult(*)(LoadJobContext&), ...>
LOADJOB_CALLBACK = 0x0082C240
LOADJOB_CALLBACK_PROLOGUE = bytes.fromhex("488bc4574154415541564157")
LOADJOB_CALLBACK_INSTALL = 0x0082AA21
LOADJOB_FUNCTOR_VTABLE_INSTALL = 0x0082AA15
LOADJOB_FUNCTOR_VTABLE = 0x02AC7728
LOADJOB_SAVE_SLOT_SIZE_LOAD = 0x0082C2D5
LOADJOB_SAVE_SLOT_SIZE_BYTES = bytes.fromhex("ba00002800")  # mov edx, 0x280000


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
    require(
        data[DISABLED_CONTINUE_BUILDER : DISABLED_CONTINUE_BUILDER + len(DISABLED_CONTINUE_BUILDER_PROLOGUE)]
        == DISABLED_CONTINUE_BUILDER_PROLOGUE,
        "disabled Continue row builder must remain 0x140764290 (idle-ctor path), not LangSelect readiness",
        failures,
    )
    require(
        rel32_call_target(data, DISABLED_CONTINUE_BUILDER_CALL) == DISABLED_CONTINUE_BUILDER,
        "title step-update method must call the disabled Continue builder at 0x140766e12",
        failures,
    )
    require(
        data[DISABLED_CONTINUE_GATE_6B0 : DISABLED_CONTINUE_GATE_6B0 + len(DISABLED_CONTINUE_GATE_6B0_BYTES)]
        == DISABLED_CONTINUE_GATE_6B0_BYTES,
        "disabled Continue build must gate on this+0x6b0 (build-request bool), not on title+0x2610 LangSelect",
        failures,
    )
    require(
        data[DISABLED_CONTINUE_GATE_6B1 : DISABLED_CONTINUE_GATE_6B1 + len(DISABLED_CONTINUE_GATE_6B1_BYTES)]
        == DISABLED_CONTINUE_GATE_6B1_BYTES,
        "disabled Continue build must also test this+0x6b1 (suppress bool); these are the real disable gates",
        failures,
    )
    require(
        rip_lea_target(data, DISABLED_CONTINUE_STEP_UPDATE_INSTALL) == DISABLED_CONTINUE_STEP_UPDATE,
        "title step object must install update method 0x140766980 (owner of the disabled Continue builder)",
        failures,
    )
    require(
        rip_lea_target(data, DISABLED_CONTINUE_STEP_VTABLE_INSTALL) == DISABLED_CONTINUE_STEP_VTABLE,
        "title step object must install step vtable 0x142a9be20 alongside the disabled Continue update method",
        failures,
    )

    builder_callers = find_rel32_callers(data, DISABLED_CONTINUE_BUILDER)
    require(
        builder_callers == [DISABLED_CONTINUE_BUILDER_CALL],
        "disabled Continue builder must have exactly one owner/caller: the title step-update method",
        failures,
    )
    require(
        rip_lea_target(data, CONTINUE_FUNC_BASE_LEA) == CONTINUE_FUNC_BASE_VTABLE
        and rip_lea_target(data, CONTINUE_FUNC_IMPL_LEA) == CONTINUE_FUNC_IMPL_VTABLE,
        "disabled Continue builder must build the _Func_base/_Func_impl<MenuWindow*,SceneProxy&> docall functor",
        failures,
    )
    require(
        rip_lea_target(data, NULL_PLAYER_MENU_CTRL_INSTALL) == NULL_PLAYER_MENU_CTRL_VTABLE,
        "title step ctor must install NullPlayerMenuCtrl at this+0x6a8 (title has no player)",
        failures,
    )
    require(
        rip_lea_target(data, BACKSCREEN_DATA_INSTALL) == BACKSCREEN_DATA_VTABLE,
        "title step ctor must install BackScreenData at this+0x710",
        failures,
    )
    require(
        data[LOADJOB_CALLBACK : LOADJOB_CALLBACK + len(LOADJOB_CALLBACK_PROLOGUE)] == LOADJOB_CALLBACK_PROLOGUE,
        "real native LoadJob callback MenuJobResult(LoadJobContext&) must remain at 0x14082c240",
        failures,
    )
    require(
        rip_lea_target(data, LOADJOB_CALLBACK_INSTALL) == LOADJOB_CALLBACK,
        "MenuJob/LoadJob factory must install the save-reading LoadJob callback 0x14082c240",
        failures,
    )
    require(
        rip_lea_target(data, LOADJOB_FUNCTOR_VTABLE_INSTALL) == LOADJOB_FUNCTOR_VTABLE,
        "LoadJob must use the _Func_impl<MenuJobResult(*)(LoadJobContext&)> functor vtable 0x142ac7728",
        failures,
    )
    require(
        data[LOADJOB_SAVE_SLOT_SIZE_LOAD : LOADJOB_SAVE_SLOT_SIZE_LOAD + len(LOADJOB_SAVE_SLOT_SIZE_BYTES)]
        == LOADJOB_SAVE_SLOT_SIZE_BYTES,
        "LoadJob must allocate/read the 0x280000-byte save-slot buffer (real save read, not a menu row)",
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
        "langselect_flags_offset=component+0x48 "
        f"disabled_continue_builder=0x{BASE + DISABLED_CONTINUE_BUILDER:x} "
        f"disabled_continue_owner=0x{BASE + DISABLED_CONTINUE_STEP_UPDATE:x} "
        f"disabled_continue_step_vtable=0x{BASE + DISABLED_CONTINUE_STEP_VTABLE:x} "
        "disabled_continue_gate=this+0x6b0&!this+0x6b1 disabled_continue_row=this+0x708 "
        f"disabled_continue_builder_callers={len(builder_callers)} "
        "step_class=CSEzUpdateTask<CSEzTask,CSMenuManImp> "
        "continue_docall_functor=_Func_impl<lambda,MenuWindow*,SceneProxy&> "
        "title_player_ctrl=NullPlayerMenuCtrl@this+0x6a8 backscreen=BackScreenData@this+0x710 "
        f"native_loadjob=0x{BASE + LOADJOB_CALLBACK:x} "
        "loadjob_type=MenuJobResult(LoadJobContext&) loadjob_save_slot_size=0x280000 "
        f"loadjob_functor_vtable=0x{BASE + LOADJOB_FUNCTOR_VTABLE:x}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
