#!/usr/bin/env python3
"""Report Elden Ring's two `CSTeamTypeRelation` matrices for a set of team ids.

Elden Ring keeps two independent 79x79 tables of `CSTeamTypeRelation*` singletons:

* `CSTeamTypeRelation_array_1` -- consulted by `CanTeamTypeDamageEachOther`
  (1.16.2 `0x14051ab40`), `canTeamTypeHitAnother` (`0x14051ac10`) and therefore by
  `getTeamTypeRelationshipWithAtkParam` (`0x14051a980`). This is the damage table.
* `CSTeamTypeRelation_array_2` -- consulted by `CS::ChrIns::CanTargetTeamType`
  (`0x14051a810`). This is the lock-on / targeting table.

They are not the same data, so "friendly for targeting" and "friendly for damage" are
separate questions and have to be answered against the right table. Both are indexed
`table[a * 79 + b]` with 8-byte pointers, where `a` is the ACTOR/attacker team and `b`
the SUBJECT/victim team.

Each cell points at one of four stateless singletons whose only member is a vptr:

    CSTeamTypeNeutral::Validate  -> only ever true for a self-target
    CSTeamTypeFriend::Validate   -> true when the attack sets `friendlyTarget`
    CSTeamTypeEnemy::Validate    -> true when the attack sets `opposeTarget`
    CSTemaTypeRival::Validate    -> true when the attack sets either  (sic, game's spelling)

An ordinary attack row carries `opposeTarget = 1, friendlyTarget = 0`, so only Enemy and
Rival cells can carry damage.

Reads the flat de-Arxan'd image (`eldenring-deobf.bin`, 1.16.2 by default); nothing is
hooked and no game is required. Point `--image` / `--base` at another build once its
table addresses are known.
"""

from __future__ import annotations

import argparse
import struct
import sys

TEAM_COUNT = 79

# 1.16.2 addresses. `CanTeamTypeDamageEachOther` loads array_1 with
# `LEA RAX,[0x143b180e0]`; `CanTargetTeamType` loads array_2 with `LEA RCX,[0x143b243f0]`.
DAMAGE_TABLE_VA = 0x143B180E0
TARGET_TABLE_VA = 0x143B243F0

# The four singletons are classified by the body of their `Validate`, not by address, so this
# reads any build. Every one opens `TEST R8B,R8B; JZ +5; MOVZX EAX,[RDX+2]; RET` (the self-target
# early-out) and then diverges -- the tail below is what actually distinguishes them:
#
#   Neutral  32 c0 c3              XOR EAX,EAX; RET                      -> never
#   Friend   0f b6 42 01 c3        MOVZX EAX,[RDX+1]; RET                -> friendlyTarget
#   Enemy    0f b6 02 c3           MOVZX EAX,[RDX];   RET                -> opposeTarget
#   Rival    80 3a 00 75 09 ...    CMP [RDX],0; JNZ ...                  -> either
RELATION_TAILS = {
    "Neutral": bytes.fromhex("32c0c3"),
    "Friend": bytes.fromhex("0fb64201c3"),
    "Enemy": bytes.fromhex("0fb602c3"),
    "Rival": bytes.fromhex("803a00750980"),
}
VALIDATE_PROLOGUE = bytes.fromhex("4584c074050fb64202c3")

# Team ids this crate and its neighbours actually care about, named from the game's own
# constant-returning accessors (`MOV byte ptr [RCX],imm8; MOV RAX,RCX; RET`).
KNOWN_TEAMS = {
    5: "WanderingPhantom (TeamType::WanderingPhantom, 0x1403f9f20)",
    13: "0x1403c7660",
    15: "Charmed (GetTeamTypeCharmed, 0x1403e9ff0)",
    21: "0x140776400",
    30: "Object (GetObjectTeamType, 0x14038b040)",
    47: "0x1404bfc50",
}


def read(image: str, base: int, va: int, size: int) -> bytes:
    with open(image, "rb") as handle:
        handle.seek(va - base)
        data = handle.read(size)
    if len(data) != size:
        raise SystemExit(f"short read at {va:#x}: wanted {size}, got {len(data)}")
    return data


def load_table(image: str, base: int, va: int) -> list[int]:
    raw = read(image, base, va, TEAM_COUNT * TEAM_COUNT * 8)
    return list(struct.unpack(f"<{TEAM_COUNT * TEAM_COUNT}Q", raw))


def classify(image: str, base: int, obj: int, cache: dict[int, str]) -> str:
    """Name the relation singleton at `obj` by disassembling its `Validate`."""
    if obj in cache:
        return cache[obj]
    if obj == 0:
        cache[obj] = "<null>"
        return cache[obj]
    vtable = struct.unpack("<Q", read(image, base, obj, 8))[0]
    validate = struct.unpack("<Q", read(image, base, vtable, 8))[0]
    body = read(image, base, validate, 24)
    name = f"<unclassified {validate:#x}>"
    if body.startswith(VALIDATE_PROLOGUE):
        tail = body[len(VALIDATE_PROLOGUE) :]
        for candidate, signature in RELATION_TAILS.items():
            if tail.startswith(signature):
                name = candidate
                break
    cache[obj] = name
    return name


def relation(image: str, base: int, table: list[int], actor: int, subject: int, cache: dict[int, str]) -> str:
    return classify(image, base, table[actor * TEAM_COUNT + subject], cache)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("teams", nargs="*", type=int, help="team ids to report (default: the known set)")
    parser.add_argument("--image", default="eldenring-deobf.bin", help="flat de-Arxan'd image")
    parser.add_argument("--base", type=lambda v: int(v, 0), default=0x140000000, help="image base VA")
    parser.add_argument("--damage-table", type=lambda v: int(v, 0), default=DAMAGE_TABLE_VA)
    parser.add_argument("--target-table", type=lambda v: int(v, 0), default=TARGET_TABLE_VA)
    parser.add_argument("--row", type=int, help="print one team's full row against every other team")
    args = parser.parse_args()

    damage = load_table(args.image, args.base, args.damage_table)
    target = load_table(args.image, args.base, args.target_table)
    cache: dict[int, str] = {}

    def pair(table: list[int], actor: int, subject: int) -> str:
        return relation(args.image, args.base, table, actor, subject, cache)

    if args.row is not None:
        print(f"team {args.row} as ACTOR, every subject team -- damage / targeting")
        for subject in range(TEAM_COUNT):
            d = pair(damage, args.row, subject)
            t = pair(target, args.row, subject)
            flag = "" if d == t else "   <- differs"
            print(f"  vs {subject:2d}: damage={d:8s} target={t:8s}{flag}")
        return 0

    teams = args.teams or sorted(KNOWN_TEAMS)
    for team in teams:
        label = KNOWN_TEAMS.get(team, "")
        print(f"\nteam {team} {label}")
        for other in teams:
            print(
                f"  {team:2d} -> {other:2d}: damage={pair(damage, team, other):8s}"
                f" target={pair(target, team, other):8s}"
            )
    return 0


if __name__ == "__main__":
    sys.exit(main())
