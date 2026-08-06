#!/usr/bin/env python3
"""Decide, in one Seamless invasion, WHICH code picks where an invader lands.

THE QUESTION
------------
Static RE of the invasion path finishes at roughly 92%. The readable part says the destination is
chosen entirely on the INVADER'S OWN MACHINE:

    GetNpcWorldInvasionInfo  0x1405ee3d0   walks the MSB `PseudoMultiplayer` regions and returns the
                                           first one geometrically containing the invader
    ReqInvadeNPCWorld        0x1409fafe0   receives that record; `+0x28` names the destination map,
                                           `+0x14` names the MSB entry point to spawn on

The unreadable part is Seamless. `ersc.dll` has its own invade site and its own spawn-position
writer, and the half that offers/accepts an invasion is inside the Themida VM. So the ONE thing
that cannot be settled by reading bytes is whether a Seamless invasion travels the vanilla path at
all -- and everything downstream depends on the answer:

  * if it does, then WHERE YOU STAND selects your destination, because the MSB region containing
    the invader is the whole input; and
  * if it does not, then the destination arrives from Seamless's own code and no amount of
    map-side filtering on our side can steer it.

WHAT THIS WATCHES, and why
--------------------------
The destination the engine places an invader at lives in `CSGameMan+0xac8`, and in 1.16.2 exactly
TWO functions write it. That bounds the whole answer space, so the run watches the writers rather
than the value:

  SetTargetMapId  0x14067acf0  <- ReqInvadeNPCWorld     this machine's own MSB-region lookup
                               <- SetMultiplayJoinData  it arrived in multiplayer join data

A return address at each write says which one decided it, and the same is done for
`SetNPCInvadeTargetEntryPoint` 0x14067ad00 (`+0xaf0`). `ReqInvadeNPCWorld` itself is hooked too,
with its whole 48-byte `NpcWorldInvasionInfo` dumped, and its CALLER named -- because one of its
three callers is `DequeueCeremonyPacket`, i.e. a received packet, which is a different story from
the local path even when the same function runs.

ERSC IS NOT HOOKED, DELIBERATELY. Its addresses are confirmed -- bytes, `.pdata` RUNTIME_FUNCTION
and unwind data all agree -- but `ersc.dll` is WinLicense/Themida-protected, and nothing in the
static image establishes whether this build verifies its own `.text`. A frida Interceptor is an
inline patch, which is exactly what such a check exists to catch, so "safe" is unproven rather
than merely unlikely to bite. Seamless's effect is fully visible from the game side anyway: if it
writes the destination, the write still goes through one of the two hooked setters, and the return
address lands in `ersc.dll` and is reported as such.

READ-ONLY. Nothing is written and nothing is called. Every hook verifies the target's first bytes
against the value established by static RE before attaching, so a version drift fails loudly
instead of attaching to the middle of some other function.

REACHING THE PROCESS
--------------------
The game runs under Wine/Proton, so a Linux-side `frida.attach()` sees nothing. `frida-gadget.dll`
is loaded into the game as an me3 `[[natives]]` entry and listens on 127.0.0.1:27042; this
connects to it as a REMOTE DEVICE. Launch with a gadget-bearing profile:

    /home/banon/Elden/pr190-invasion-warp-seamless-frida.me3

RUN IT (frida is provisioned per-run by uv; nothing is installed system-wide):

    uv run --with frida python3 \
        /home/banon/projects/er-effects-rs/scripts/frida-invasion-mechanism-trace.py

SELFTEST (no game, no frida -- proves the verdict logic):

    python3 /home/banon/projects/er-effects-rs/scripts/frida-invasion-mechanism-trace.py --selftest
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import threading

GADGET = "127.0.0.1:27042"
DEFAULT_OUT = (
    "/tmp/claude-1000/-home-banon-projects-er-effects-rs/"
    "fdd5f467-bf36-402d-bbcd-6defe1f4d0b7/scratchpad/invasion-mechanism-trace.jsonl"
)

# ---------------------------------------------------------------------------------------------
# THE VERDICT. This is the whole point of the run, so it lives in plain python and is covered by
# --selftest: a run that produces records but no conclusion has wasted a launch.
# ---------------------------------------------------------------------------------------------

#: The destination was written by ReqInvadeNPCWorld -- decided on this machine, from the MSB
#: PseudoMultiplayer region the invader was standing in. This is the steerable case.
LOCAL_MSB = "local-msb-lookup"
#: The destination was written by SetMultiplayJoinData -- it arrived in multiplayer join data, so
#: it was decided elsewhere and this machine only received it.
JOIN_DATA = "arrived-in-join-data"
#: The destination was written from ersc.dll itself.
ERSC_OWNS_IT = "ersc-wrote-it-directly"
#: The destination changed but nothing we watch wrote it -- a writer we have not identified.
UNKNOWN_WRITER = "unknown-writer"
#: Nothing observed. Not a finding -- a run that did not capture an invasion.
NO_INVASION = "no-invasion-observed"

#: `SetTargetMapId` is the field the engine places from, so its LAST writer before placement is the
#: one that actually decided the destination. Its two callers in 1.16.2 bound the answer space.
_WRITER_VERDICT = {
    "ReqInvadeNPCWorld": LOCAL_MSB,
    "SetMultiplayJoinData": JOIN_DATA,
}

def _armed_writes(records: list[dict]) -> list[dict]:
    """The `SetTargetMapId` writes that name a destination the engine can actually place from.

    RETRACTED PREMISE, 2026-08-06. This filter first required a NON-ZERO entry point, on the
    reasoning that 0 means "unset". Decompiling `SetMultiplayJoinData` (0x1406fb520) killed that:
    on the join path the entry point is zeroed UNCONDITIONALLY --

        SetTargetMapId(local_68);
        SetMultiplayJoinTargetBlockPos(&local_28);      // spawnPosition x/y/z
        local_68[0] = {0,0,0,0};
        SetNPCInvadeTargetEntryPoint((int *)local_68);  // always zero, by construction

    -- because that path does not place by MSB entry point at all. It places at a raw COORDINATE.
    So a zero entry point is definitional there, not evidence of an uncommitted candidate, and the
    old filter would have discarded every real server-pushed destination.

    What actually distinguishes a placeable destination is therefore the accompanying SPAWN
    COORDINATE, not the entry point: `ReqInvadeNPCWorld` arms an entry point and no coordinate,
    `SetMultiplayJoinData` arms a coordinate and no entry point, and either is sufficient.
    """
    def armed_by(name, pred):
        return [r for r in records
                if r.get("type") == "writer" and r.get("name") == name and pred(r)]

    entries = armed_by("SetNPCInvadeTargetEntryPoint", lambda r: r.get("value") not in (0, -1))
    coords = [r for r in records if r.get("type") == "spawn-pos"]
    marks = sorted(r.get("seq", 0) for r in entries + coords)
    out = []
    for w in records:
        if w.get("type") != "writer" or w.get("name") != "SetTargetMapId":
            continue
        # Either arming signal may follow the map write; the engine writes the map first.
        if any(m >= w.get("seq", 0) for m in marks):
            out.append(w)
    return out


def classify(records: list[dict]) -> dict:
    """Reduce a run's records to the one answer the run exists to produce.

    The verdict keys on WHO WROTE THE DESTINATION, not on what the destination was: every case
    looks identical on screen ("I invaded someone"), and the follow-up work is completely
    different for each. Kept out of the frida plumbing so it is checkable without a game.
    """
    req = [r for r in records if r.get("type") == "req-invade"]
    all_map_writes = [r for r in records
                      if r.get("type") == "writer" and r.get("name") == "SetTargetMapId"]
    writes = _armed_writes(records)
    placed = [r for r in records if r.get("type") == "placement"]

    if not req and not all_map_writes and not placed:
        return {"verdict": NO_INVASION, "why": "no invasion request, destination write or placement was seen"}

    # Map writes that were never armed are candidates/joins, not invasions. Counted and reported,
    # because "Seamless offered me 14 destinations and committed to none" is a real observation --
    # but they must not be scored as the destination.
    unarmed = len(all_map_writes) - len(writes)
    if not writes and not placed and unarmed:
        return {
            "verdict": NO_INVASION,
            "why": (
                f"{unarmed} destination map write(s) seen, every one with an UNSET entry point, so "
                "these are session-join/candidate writes and no invasion destination was committed"
            ),
            "candidate_maps": [w.get("block") for w in all_map_writes],
            "candidate_writers": sorted({w.get("caller_name") for w in all_map_writes}),
        }

    if not writes:
        # A destination that changed with no write observed is a gap in the hooks, NOT evidence
        # about who decided it. Saying anything stronger would be inventing a finding.
        if placed:
            return {
                "verdict": UNKNOWN_WRITER,
                "why": (
                    "the placement fields changed but neither SetTargetMapId hook fired, so the "
                    "writer is unidentified -- treat the hooks as incomplete, not the path as proven"
                ),
                "placements": placed,
            }
        return {
            "verdict": NO_INVASION,
            "why": "the invasion request fired but no destination was ever written",
            "requests": req,
        }

    last = writes[-1]
    caller = last.get("caller_name") or ""
    verdict = _WRITER_VERDICT.get(caller)
    if verdict is None:
        verdict = ERSC_OWNS_IT if caller.startswith("ersc.dll") else UNKNOWN_WRITER
    out = {
        "verdict": verdict,
        "why": f"the last ARMED destination write came from {caller or 'an unnamed address'}",
        "destination": last.get("block"),
        "writer": last,
        "requests_seen": len(req),
        "unarmed_candidate_writes": unarmed,
    }
    # DID IT ACTUALLY LAND. "A destination was pushed" and "you went there" are separate claims,
    # and only the second one proves an invasion happened. The signature is `lastLoadPosition`
    # becoming the pushed coordinate IN the pushed map.
    #
    # Both halves are required. `lastLoadPosition` is a GENERAL load field -- warping home, fast
    # travel and an invasion join all write it (observed 2026-08-06: a warp home immediately after
    # a committed invasion rewrote it with the player's own position). Position equality alone
    # would therefore credit any load that happened to be sampled; requiring the MAP to match too
    # is what keeps an unrelated load from reading as an invasion landing.
    commits = [p for p in placed
               if p.get("join_pos") and p.get("join_pos") == p.get("last_load_pos")
               and p.get("block") == last.get("block")]
    out["committed"] = bool(commits)
    out["landed_at"] = commits[-1].get("join_pos") if commits else None
    if not commits:
        out["why"] += "; NO commit observed -- a destination was pushed but nothing shows you went"
    # When the vanilla request DID run, say whether the destination that stuck is the one it asked
    # for. A request that fired and was then overwritten is a materially different situation from
    # one that fired and won, and both produce verdict LOCAL_MSB on the writer alone.
    if req:
        asked = req[-1]
        out["requested_by_reqinvade"] = asked.get("ceremony_block")
        out["request_survived"] = (
            asked.get("ceremony_block") == last.get("block")
            and asked.get("seq", 0) <= last.get("seq", 0)
        )
        out["request_caller"] = asked.get("caller_name")
    return out


def _selftest() -> int:
    fails = 0

    def check(ok: bool, label: str) -> None:
        nonlocal fails
        print(f"  {'ok  ' if ok else 'FAIL'} {label}")
        if not ok:
            fails += 1

    def write(seq, caller, block="m60_51_36_00", entry=1042380):
        """A map write plus the entry-point write that arms (or does not arm) it."""
        return [
            {"type": "writer", "name": "SetTargetMapId", "seq": seq,
             "caller_name": caller, "block": block},
            {"type": "writer", "name": "SetNPCInvadeTargetEntryPoint", "seq": seq + 1,
             "caller_name": caller, "value": entry},
        ]

    def place(seq, block, join, last):
        return [{"type": "placement", "seq": seq, "block": block,
                 "join_pos": join, "last_load_pos": last}]

    def pos(seq, caller="SetMultiplayJoinData"):
        return [{"type": "spawn-pos", "seq": seq, "caller_name": caller,
                 "x": 100.0, "y": 20.0, "z": -300.0}]

    # COMMIT DETECTION, and the case that nearly produced a false retraction (2026-08-06).
    # A committed invasion, then a WARP HOME. The warp rewrites lastLoadPosition with the player's
    # own position, so the final sample no longer matches -- but the invasion still happened. The
    # commit must be found anywhere in the run, not only in the last sample.
    D = [49.6, 302.5, -45.6]
    HOME = [-23.6, 16.3, -57.5]
    warped = classify(
        write(4, "SetMultiplayJoinData", "m61_46_43_00", entry=0) + pos(5)
        + place(6, "m61_46_43_00", D, HOME)   # staged
        + place(7, "m61_46_43_00", D, D)      # committed -- you are there
        + place(8, "m61_46_43_00", D, HOME)   # user warps home afterwards
    )
    check(warped["committed"] and warped["landed_at"] == D,
          "a commit followed by a warp home is still a commit")

    # A destination pushed but never reached must NOT read as a landing.
    staged_only = classify(
        write(4, "SetMultiplayJoinData", "m61_46_43_00", entry=0) + pos(5)
        + place(6, "m61_46_43_00", D, HOME)
    )
    check(not staged_only["committed"] and staged_only["landed_at"] is None,
          "a pushed destination you never reached is not a landing")

    # Position equality in a DIFFERENT map is an unrelated load, not this invasion landing.
    check(
        not classify(
            write(4, "SetMultiplayJoinData", "m61_46_43_00", entry=0) + pos(5)
            + place(6, "m60_51_36_00", D, D)
        )["committed"],
        "matching positions in another map are an unrelated load, not a landing",
    )

    # THE RETRACTION, kept as a test so it cannot come back. `SetMultiplayJoinData` zeroes the entry
    # point unconditionally and places by COORDINATE instead, so a zero entry point there is
    # definitional -- not evidence of an uncommitted candidate. A filter that required a non-zero
    # entry point discarded every real server-pushed destination and reported 'no invasion'.
    server_push = classify(
        write(4, "SetMultiplayJoinData", "m60_51_43_00", entry=0) + pos(6)
    )
    check(server_push["verdict"] == JOIN_DATA,
          "a zero entry point WITH a spawn coordinate is a real server-pushed destination")

    # The entry-point path arms with no coordinate, and must still be scored.
    check(
        classify(write(2, "ReqInvadeNPCWorld", entry=1042380))["verdict"] == LOCAL_MSB,
        "an entry point with no coordinate is a real destination too",
    )

    # A map write with NEITHER arming signal is genuinely not a destination.
    bare = classify(write(4, "SetMultiplayJoinData", "m11_00_00_00", entry=0))
    check(bare["verdict"] == NO_INVASION,
          "a map write with no entry point and no coordinate is not a destination")
    check(bare["candidate_maps"] == ["m11_00_00_00"],
          "the unarmed map is still reported rather than dropped")

    # An empty run must not read as a finding. This is the failure mode that matters most: a
    # clean-looking run with no invasion in it is NOT evidence about which path was used.
    check(classify([])["verdict"] == NO_INVASION, "an empty run is 'no invasion', never a verdict")
    check(
        classify([{"type": "hook", "ok": True}])["verdict"] == NO_INVASION,
        "plumbing records alone are still 'no invasion'",
    )

    # The steerable case: the destination was written by the local MSB-region lookup.
    check(
        classify(write(2, "ReqInvadeNPCWorld"))["verdict"] == LOCAL_MSB,
        "a destination written by ReqInvadeNPCWorld is the local MSB lookup",
    )
    # The unsteerable case: it arrived over the wire.
    check(
        classify(write(2, "SetMultiplayJoinData"))["verdict"] == JOIN_DATA,
        "a destination written by SetMultiplayJoinData arrived in join data",
    )
    check(
        classify(write(2, "ersc.dll+0x8e711"))["verdict"] == ERSC_OWNS_IT,
        "a destination written from inside ersc.dll is ersc's own",
    )

    # An unrecognised writer must NOT be rounded to the nearest known one -- that would report a
    # mechanism the run never observed.
    check(
        classify(write(2, "unknown@0x141234567"))["verdict"] == UNKNOWN_WRITER,
        "an unrecognised writer is reported as unknown, not guessed",
    )

    # A placement with no write observed is a HOLE IN THE HOOKS, not evidence about the path. This
    # is the trap the earlier design fell into: absence of our hook firing is not presence of ersc.
    check(
        classify([{"type": "placement", "seq": 4, "block": "m60_51_36_00", "entry_point": 1042380}])[
            "verdict"
        ]
        == UNKNOWN_WRITER,
        "a placement with no observed write means the hooks are incomplete, not that ersc did it",
    )

    # The LAST write decides, because that is the value placement actually uses.
    check(
        classify(write(2, "ReqInvadeNPCWorld") + write(5, "SetMultiplayJoinData"))["verdict"]
        == JOIN_DATA,
        "the last destination write is the one that decided it",
    )

    # Request fired AND won: the destination that stuck is the one it asked for.
    survived = classify(
        [
            {"type": "req-invade", "seq": 1, "ceremony_block": "m60_51_36_00",
             "caller_name": "ReqInvadeNpcWorld"},
        ]
        + write(2, "ReqInvadeNPCWorld")
    )
    check(survived["verdict"] == LOCAL_MSB and survived["request_survived"],
          "a request whose destination stuck is reported as having survived")

    # Request fired and was then OVERWRITTEN. Same verdict family is not enough -- the run must say
    # the request lost, or an overridden invasion reads as a working one.
    overwritten = classify(
        [
            {"type": "req-invade", "seq": 1, "ceremony_block": "m60_51_36_00",
             "caller_name": "ReqInvadeNpcWorld"},
        ]
        + write(2, "ReqInvadeNPCWorld")
        + write(4, "SetMultiplayJoinData", "m60_42_39_00")
    )
    check(
        overwritten["verdict"] == JOIN_DATA and not overwritten["request_survived"],
        "a request that was overwritten is reported as not having survived",
    )

    # A write recorded BEFORE the request is the previous invasion's, and must not be credited to
    # this one -- that would manufacture a false 'the request survived'.
    stale = classify(
        [
            {"type": "req-invade", "seq": 4, "ceremony_block": "m60_51_36_00",
             "caller_name": "ReqInvadeNpcWorld"},
        ]
        + write(1, "ReqInvadeNPCWorld")
    )
    check(not stale["request_survived"], "a write older than the request cannot confirm it")

    # The caller of the request is carried through, because DequeueCeremonyPacket vs
    # ReqInvadeNpcWorld is the difference between a packet asking and this machine asking.
    check(
        classify(
            [
                {"type": "req-invade", "seq": 1, "ceremony_block": "m60_51_36_00",
                 "caller_name": "DequeueCeremonyPacket"},
            ]
            + write(2, "ReqInvadeNPCWorld")
        )["request_caller"]
        == "DequeueCeremonyPacket",
        "the request's own caller is reported, not just the writer's",
    )

    if fails:
        print(f"selftest FAILED ({fails})")
        return 1
    print("selftest ok")
    return 0


AGENT = r"""
// ---------------------------------------------------------------------------------------------
// ADDRESSES. Every one of these is a claim about someone else's process, so each is verified
// against the bytes actually present before anything is attached, and a mismatch REFUSES rather
// than hooking whatever now lives there.
//
// eldenring.exe 1.16.2: the Ghidra dump VA, eldenring-deobf.bin's VA and the live VA are all the
// same number (shift zero, image base 0x140000000), so these are written as VAs and turned into
// offsets against the module's real base at runtime.
// ---------------------------------------------------------------------------------------------
const ER_IMAGE_BASE = ptr('0x140000000');

const ER = {
  // CS::PartyMemberInfo::ReqInvadeNPCWorld -- the vanilla invasion request.
  //   bool ReqInvadeNPCWorld(PartyMemberInfo *pmi, NpcWorldInvasionInfo *info, uint *outMapNumber)
  // so the record we want is ARG 1 (rdx). Read out of the 1.16.2 dump's own signature, not assumed.
  // Prologue read independently out of eldenring-deobf.bin at file offset 0x9fafe0:
  //   push rbp; push rsi; push rdi; push r12; push r13; push r14; push r15; sub rsp,0x50
  // and the bytes immediately before it are the previous function's `jmp`, so this is a boundary.
  reqInvade: { va: '0x1409fafe0', prologue: '4055565741544155415641574883ec50', infoArg: 1 },
};

// NpcWorldInvasionInfo -- 48 bytes, every field name and offset from the 1.16.2 dump's own typed
// structure (getStructure), not inferred from access patterns.
const INFO_FIELDS = [
  ['onlineEnabled',           0x00, 'u8'],
  ['hostEntityId',            0x04, 's32'],
  ['eventFlag',               0x08, 'u32'],
  ['activateGoods',           0x0c, 's32'],
  ['ceremony',                0x10, 's32'],
  ['hostEntryPointEntityId',  0x14, 's32'],
  ['coop1EntryPointEntityId', 0x18, 's32'],
  ['coop2EntryPointEntityId', 0x1c, 's32'],
  ['ceremonyType',            0x20, 'u8'],
  ['ceremonyNetworkMsgNpc',   0x21, 'u8'],
  ['eventTextForMapFmgId',    0x24, 's32'],
  ['ceremonyParamId',         0x28, 'u32'],
  ['roleParamIdOverride',     0x2c, 's32'],
];

// CSGameMan singleton pointer, and the two fields the engine PLACES from. Byte-verified in this
// repo against their own named getters (crates/er-invasion-warp/src/seamless_invade_probe.rs):
//   +0xac8 destination BlockId          GetTargetMapId                0x140679630
//   +0xaf0 npcInvadeTargetEntryPoint    GetNpcInvadeTargetEntryPoint  0x140679650
// The whole placement block, field names straight from the 1.16.2 dump's GameMan structure. THREE
// different mechanisms write into it, at three different offsets, which is why the poll reads all
// of them rather than just the map:
//   +0xaa0 lastLoadPosition             <- ersc.dll's own detour writes HERE, last, bypassing the
//                                          multiplayJoinTarget fields entirely
//   +0xac8 loadTargetMapId              <- ReqInvadeNPCWorld and SetMultiplayJoinData
//   +0xacc multiplayJoinTargetBlockPos  <- SetMultiplayJoinData only (the server-pushed coordinate)
//   +0xaf0 npcInvadeTargetEntryPoint    <- ReqInvadeNPCWorld only; join path zeroes it
// Polling +0xaa0 is how ersc's effect is observed WITHOUT hooking a Themida-protected page.
const GAME_MAN_PTR_VA = '0x143d69918';
const LAST_LOAD_POSITION_OFFSET = 0xaa0;
const TARGET_MAP_ID_OFFSET = 0xac8;
const JOIN_TARGET_POS_OFFSET = 0xacc;
const ENTRY_POINT_OFFSET = 0xaf0;
const RAPID_REENTRY_OFFSET = 0xafc;
const ENTRY_POINT_NONE = -1;
const BLOCK_NONE = 0xffffffff;

// THE TWO WRITERS OF THE DESTINATION, hooked instead of anything inside ersc.dll.
//
// SetTargetMapId (+0xac8) and SetNPCInvadeTargetEntryPoint (+0xaf0) each have exactly TWO callers
// in 1.16.2 -- ReqInvadeNPCWorld and SetMultiplayJoinData@0x1406fb520 -- so watching them bounds
// the entire answer space: a destination either came from this machine's own MSB lookup, or it
// arrived in multiplayer JOIN DATA. The return address says which, every time.
//
// WHY NOT HOOK ersc.dll. Its addresses are confirmed (bytes, .pdata RUNTIME_FUNCTION and unwind
// data all agree), but ersc.dll is WinLicense/Themida-protected -- a 5.3 MB RWX `.themida` section,
// the PE exception directory rebuilt inside it, "WinLicenseVersion" embedded in the entry stub --
// and NOTHING in the static image establishes whether this build enables memory-integrity checking
// over `.text`. A frida Interceptor is exactly the inline patch such a check exists to catch, so
// "safe" is unproven, not merely unlikely to bite. There is also an unresolved hook-ordering
// collision: ersc installs its 0x8f4b0 routine as the detour over the game's spawn-position
// selector FUN_140afcf60, which this repo's own coordinate-free warp already depends on.
// Watching ersc's EFFECT from the game side costs nothing and touches no protected page.
//
// Each is a 16-byte leaf: mov rax,[rip+..] ; mov edx,[rcx] ; mov [rax+off],edx ; ret -- which is
// also where the +0xac8/+0xaf0 offsets are confirmed for the third independent time.
const WRITERS = [
  { name: 'SetTargetMapId',               va: '0x14067acf0',
    prologue: '488b0521ec6e038b118990c80a0000c3' },
  { name: 'SetNPCInvadeTargetEntryPoint', va: '0x14067ad00',
    prologue: '488b0511ec6e038b118990f00a0000c3' },
];

// THE WHOLE CANDIDATE RECORD. `SetMultiplayJoinData(SosSignMan*, ServerPushJoinData*)` receives
// everything the server sent about this candidate, 0x80 bytes, BEFORE anything is committed. This
// is the record a location filter would judge, so it is dumped in full rather than field by field:
// what a filter CAN match on is the question, and a field we did not think to decode is exactly
// the kind of thing that answers it.
//
// Layout from the 1.16.2 dump's own ServerPushJoinData structure:
//   +0x00 u32          "matchPlayerCount" -- MISNAMED in the dump: this is the value handed to
//                       SetTargetMapId, i.e. the destination BlockId. Decoded as both.
//   +0x04 FloatVector3 spawnPosition          +0x10 float spawnAngle
//   +0x14 int          entryfilelistId        +0x18 SummonParamType summonParamType
//   +0x1c MultiplayRole multiplayRole         +0x1d bool hasPassword
//   +0x1e bool         shouldUseRapidReentry  +0x20 FnVector<byte> lobbyData (begin/end/cap)
//   +0x38 byte         settings               +0x39 byte stage
//   +0x3a wchar_t[32]  password
const JOIN_DATA_FN = { name: 'SetMultiplayJoinData', va: '0x1406fb520',
                       prologue: '40534881ec8000000048c7442428feff' };
const JOIN_DATA_LEN = 0x80;

// THE COORDINATE. `SetMultiplayJoinData` places by raw position, not by MSB entry point, so on the
// server-pushed path this -- not the entry point -- is what says a destination is real:
//   void SetMultiplayJoinTargetBlockPos(FloatVector3*) -> GameMan->multiplayJoinTargetBlockPos
const SPAWN_POS = { name: 'SetMultiplayJoinTargetBlockPos', va: '0x14067ad10',
                    prologue: '488b1501ec6e03f20f1001f20f1182cc' };

let seq = 0;
function emit(o) { o.seq = ++seq; send(o); }

// eldenring.exe's load base minus its preferred base. Every VA constant above is a PREFERRED-base
// address and must have this added before it is dereferenced. Module scope, not install()-local,
// because readPlacement() runs from the poll timer as well as from inside the hooks.
let slide = ptr(0);

function hexBytes(ptr_, len) {
  try {
    const b = ptr_.readByteArray(len);
    if (b === null) return null;
    return Array.from(new Uint8Array(b))
      .map(function (x) { return ('0' + x.toString(16)).slice(-2); }).join('');
  } catch (e) { return null; }
}

// m{AA}_{BB}_{CC}_{DD} -- the engine's own debug spelling of a BlockId, whose bytes in memory are
// [index, region, block, area] low-to-high. The index byte is BCD-packed for areas 50..88, which
// is why it is decoded rather than printed raw: an overworld block with a two-digit index prints
// wrong otherwise. Mirrors BlockKey in crates/er-invasion-warp/src/invasion_warp.rs.
function blockName(raw) {
  if (raw === BLOCK_NONE) return 'none';
  const area = (raw >>> 24) & 0xff, block = (raw >>> 16) & 0xff;
  const region = (raw >>> 8) & 0xff, packed = raw & 0xff;
  const index = (area >= 50 && area <= 88) ? ((packed >> 4) * 10 + (packed & 0x0f)) : packed;
  function d2(n) { return ('0' + n).slice(-2); }
  return 'm' + d2(area) + '_' + d2(block) + '_' + d2(region) + '_' + d2(index);
}

// ceremonyParamId -> destination map, exactly as ReqInvadeNPCWorld itself decodes it:
//     if ((int)id < 1)  -> FieldArea::GetCurrentMapId    (stay in the map you are already in)
//     else              -> MakeBlockId(id/1000000%100, id/10000%100, id/100%100, id%100)
// i.e. the id is DECIMAL AABBCCDD, which is a different encoding from the packed-byte BlockId
// above. Conflating the two is how a destination gets misreported as a completely different map.
function ceremonyBlockName(v) {
  if ((v | 0) < 1) return 'current-map';
  function d2(n) { return ('0' + n).slice(-2); }
  return 'm' + d2(Math.floor(v / 1000000) % 100) + '_' + d2(Math.floor(v / 10000) % 100)
       + '_' + d2(Math.floor(v / 100) % 100) + '_' + d2(v % 100);
}

// The three functions that call ReqInvadeNPCWorld in 1.16.2, with their body extents from the
// dump. A return address inside one of them names WHO asked for the invasion, which is the single
// most informative field in the record: DequeueCeremonyPacket means a received packet drove it.
// Anything outside all three is reported as unknown rather than guessed -- an unrecognised caller
// (ersc.dll, say) is itself the answer, and must not be silently rounded to the nearest known one.
// Written as STRINGS: these exceed 2^32, and handing a large JS Number to ptr() is exactly where a
// silently-truncated address comes from.
const CALLERS = [
  // -- callers of ReqInvadeNPCWorld --
  ['ReqInvadeNpcWorld',     '0x1405f1940', '0x1405f1a42'],
  ['WarpNextStage_Bonfire', '0x140595170', '0x1405953a9'],
  ['DequeueCeremonyPacket', '0x1409fd400', '0x1409fd54b'],
  // -- the other writer of the destination, and the request that reaches it --
  ['ReqInvadeNPCWorld',     '0x1409fafe0', '0x1409fb685'],
  ['SetMultiplayJoinData',  '0x1406fb520', '0x1406fb605'],
  // Seamless replaces the call to ReqInvadeNpcWorld inside this one, at 0x14065add2.
  ['UpdateMultiplayData',   '0x14065a930', '0x14065c016'],
];
// Takes the RAW return address. Each module is normalised against ITS OWN base: subtracting
// eldenring.exe's slide from an address that is actually inside ersc.dll would compare two
// unrelated numbers and mislabel the caller, which is the one field the verdict turns on.
function namedCaller(raw) {
  const erVa = raw.sub(slide);
  for (let i = 0; i < CALLERS.length; i++) {
    if (erVa.compare(ptr(CALLERS[i][1])) >= 0 && erVa.compare(ptr(CALLERS[i][2])) <= 0) {
      return CALLERS[i][0];
    }
  }
  const mod = Process.findModuleByAddress(raw);
  if (mod !== null) {
    return mod.name + '+0x' + raw.sub(mod.base).toString(16);
  }
  return 'unknown@' + raw.toString();
}

function readInfo(p) {
  const out = {};
  for (let i = 0; i < INFO_FIELDS.length; i++) {
    const name = INFO_FIELDS[i][0], off = INFO_FIELDS[i][1], kind = INFO_FIELDS[i][2];
    try {
      const a = p.add(off);
      out[name] = kind === 'u8' ? a.readU8() : (kind === 'u32' ? a.readU32() : a.readS32());
    } catch (e) { out[name] = null; }
  }
  return out;
}

// Verify-then-attach. Refusing on a prologue mismatch is the difference between a failed trace and
// a corrupted game: attaching to a drifted address rewrites instructions in a live process.
function attachVerified(addr, expectHex, name, onEnter) {
  if (!expectHex || expectHex.indexOf('__') === 0) {
    emit({ type: 'hook', ok: false, name: name, detail: 'no verified prologue constant; refusing' });
    return false;
  }
  const got = hexBytes(addr, expectHex.length / 2);
  if (got !== expectHex) {
    emit({ type: 'hook', ok: false, name: name, detail: 'prologue mismatch',
           expected: expectHex, got: got });
    return false;
  }
  Interceptor.attach(addr, { onEnter: onEnter });
  emit({ type: 'hook', ok: true, name: name, detail: addr.toString() });
  return true;
}

// The placement fields, read straight out of CSGameMan.
function readPlacement() {
  try {
    const gm = ptr(GAME_MAN_PTR_VA).add(slide).readPointer();
    if (gm.isNull()) return null;
    const raw = gm.add(TARGET_MAP_ID_OFFSET).readU32();
    const entry = gm.add(ENTRY_POINT_OFFSET).readS32();
    function vec(off) {
      return [gm.add(off).readFloat(), gm.add(off + 4).readFloat(), gm.add(off + 8).readFloat()];
    }
    const joinPos = vec(JOIN_TARGET_POS_OFFSET);
    const lastLoad = vec(LAST_LOAD_POSITION_OFFSET);
    const nonZero = function (v) { return v[0] !== 0 || v[1] !== 0 || v[2] !== 0; };
    return {
      block_raw: raw, block: blockName(raw), entry_point: entry,
      join_pos: joinPos, last_load_pos: lastLoad,
      rapid_reentry: gm.add(RAPID_REENTRY_OFFSET).readU8(),
      // ARMED means "the engine could place from this". Either mechanism suffices: the local path
      // sets an entry point and no coordinate, the server-pushed path sets a coordinate and zeroes
      // the entry point. Requiring the entry point alone -- as this did until the SetMultiplayJoinData
      // decompile corrected it -- can never see a server-pushed destination at all.
      armed: raw !== BLOCK_NONE
             && ((entry !== ENTRY_POINT_NONE && entry !== 0) || nonZero(joinPos)),
    };
  } catch (e) { return null; }
}

// Only a CHANGE is reported. The fields persist after an invasion, so polling them raw would
// re-report the previous invasion's destination forever and manufacture a 'placement' that this
// run never caused.
let lastPlacement = null;
function pollPlacement() {
  const p = readPlacement();
  if (p === null || !p.armed) return;
  // lastLoadPosition is part of the key on purpose: ersc writes THERE and nowhere else we hook, so
  // a change with the map and entry point unchanged is precisely the signature of ersc placing you.
  const key = p.block_raw + ':' + p.entry_point + ':' + p.join_pos.join(',')
            + ':' + p.last_load_pos.join(',');
  if (key === lastPlacement) return;
  lastPlacement = key;
  p.type = 'placement';
  emit(p);
}

rpc.exports = {
  install: function () {
    const er = Process.findModuleByName('eldenring.exe');
    if (er === null) return { ok: false, why: 'eldenring.exe not found' };
    slide = er.base.sub(ER_IMAGE_BASE);
    const out = { ok: true, er_base: er.base.toString(), slide: slide.toString(), hooked: [] };

    const reqAddr = ptr(ER.reqInvade.va).add(slide);
    if (attachVerified(reqAddr, ER.reqInvade.prologue, 'ReqInvadeNPCWorld', function (args) {
      // The NpcWorldInvasionInfo record, dumped WHOLE as well as decoded. Keeping the raw 48 bytes
      // means a field we have mis-read stays recoverable from the log instead of being lost.
      const info = args[ER.reqInvade.infoArg];
      const rec = { type: 'req-invade', pmi: args[0].toString(), info: info.toString(),
                    raw: hexBytes(info, 0x30), fields: readInfo(info) };
      rec.host_entry_point = rec.fields.hostEntryPointEntityId;
      rec.ceremony_param_id = rec.fields.ceremonyParamId;
      rec.ceremony_block = ceremonyBlockName(rec.fields.ceremonyParamId);
      // WHO ASKED. ReqInvadeNPCWorld has three callers in 1.16.2 -- ReqInvadeNpcWorld@1405f1940,
      // WarpNextStage_Bonfire@140595170, and DequeueCeremonyPacket@1409fd400. The last one is fed
      // by a received PACKET, so the caller is what separates "this machine chose the destination"
      // from "the destination arrived over the wire". That distinction is the run's whole point,
      // and it is invisible without the return address.
      rec.caller = this.returnAddress.toString();
      rec.caller_name = namedCaller(this.returnAddress);
      rec.placement_at_request = readPlacement();
      emit(rec);
    })) out.hooked.push('ReqInvadeNPCWorld');

    // Both destination writers. The VALUE is read from the argument rather than from CSGameMan,
    // because the setter has not stored it yet at onEnter -- reading the field here would report
    // the PREVIOUS destination and quietly invert the finding.
    WRITERS.forEach(function (w) {
      const addr = ptr(w.va).add(slide);
      if (attachVerified(addr, w.prologue, w.name, function (args) {
        const rec = { type: 'writer', name: w.name, arg: args[0].toString(),
                      caller: this.returnAddress.toString(),
                      caller_name: namedCaller(this.returnAddress) };
        try {
          const v = args[0].readU32();
          rec.value = v;
          if (w.name === 'SetTargetMapId') rec.block = blockName(v);
        } catch (e) { rec.value = null; }
        emit(rec);
      })) out.hooked.push(w.name);
    });

    // The candidate record, dumped before any of the setters below run.
    const jdAddr = ptr(JOIN_DATA_FN.va).add(slide);
    if (attachVerified(jdAddr, JOIN_DATA_FN.prologue, JOIN_DATA_FN.name, function (args) {
      const p = args[1];
      const rec = { type: 'candidate', ptr: p.toString(), raw: hexBytes(p, JOIN_DATA_LEN) };
      try {
        const mapRaw = p.readU32();
        rec.map_raw = mapRaw;
        rec.map = blockName(mapRaw);
        rec.spawn = [p.add(0x04).readFloat(), p.add(0x08).readFloat(), p.add(0x0c).readFloat()];
        rec.spawn_angle = p.add(0x10).readFloat();
        rec.entryfilelist_id = p.add(0x14).readS32();
        rec.summon_param_type = p.add(0x18).readS32();
        rec.multiplay_role = p.add(0x1c).readU8();
        rec.has_password = p.add(0x1d).readU8() !== 0;
        rec.rapid_reentry = p.add(0x1e).readU8() !== 0;
        rec.settings = p.add(0x38).readU8();
        rec.stage = p.add(0x39).readU8();
        // wchar_t[32]; readUtf16String stops at the NUL rather than running to the fixed width.
        rec.password = p.add(0x3a).readUtf16String();
      } catch (e) { rec.decode_error = '' + e; }
      // lobbyData is a begin/end/cap triple. Whatever identifies the HOST would live in this
      // payload, so it is followed one level -- bounded, because a garbage length here would
      // otherwise try to read an arbitrary span out of a live process.
      try {
        const begin = p.add(0x20).readPointer();
        const end = p.add(0x28).readPointer();
        const span = end.sub(begin).toInt32();
        rec.lobby_data_len = span;
        if (span > 0 && span <= 0x400) rec.lobby_data = hexBytes(begin, span);
      } catch (e) { rec.lobby_data_len = null; }
      emit(rec);
    })) out.hooked.push(JOIN_DATA_FN.name);

    const posAddr = ptr(SPAWN_POS.va).add(slide);
    if (attachVerified(posAddr, SPAWN_POS.prologue, SPAWN_POS.name, function (args) {
      const rec = { type: 'spawn-pos', caller: this.returnAddress.toString(),
                    caller_name: namedCaller(this.returnAddress) };
      try {
        rec.x = args[0].readFloat();
        rec.y = args[0].add(4).readFloat();
        rec.z = args[0].add(8).readFloat();
      } catch (e) { rec.read_error = '' + e; }
      emit(rec);
    })) out.hooked.push(SPAWN_POS.name);

    setInterval(pollPlacement, 250);
    return out;
  },

  snapshot: function () { return readPlacement(); },
};
"""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gadget", default=GADGET)
    parser.add_argument("--out", default=DEFAULT_OUT)
    parser.add_argument("--selftest", action="store_true", help="prove the verdict logic; no game")
    args = parser.parse_args()

    if args.selftest:
        return _selftest()

    repo = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if os.path.abspath(args.out).startswith(repo):
        print(f"REFUSING: {args.out} is inside the repo.", file=sys.stderr)
        return 2

    try:
        sys.stdout.reconfigure(line_buffering=True)
    except (AttributeError, ValueError):
        pass

    try:
        import frida
    except ImportError:
        print(
            "ERROR: frida is not importable. uv provisions it per-run:\n"
            "  uv run --with frida python3 "
            "/home/banon/projects/er-effects-rs/scripts/frida-invasion-mechanism-trace.py",
            file=sys.stderr,
        )
        return 7

    try:
        device = frida.get_device_manager().add_remote_device(args.gadget)
        session = device.attach("Gadget")
    except Exception as exc:
        print(
            f"ERROR: could not reach frida-gadget at {args.gadget}: {exc}\n"
            "Is the game running with a gadget-bearing profile?\n"
            "  /home/banon/Elden/pr190-invasion-warp-seamless-frida.me3",
            file=sys.stderr,
        )
        return 3

    script = session.create_script(AGENT)
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    handle = open(args.out, "w", encoding="utf-8")
    records: list[dict] = []
    done = threading.Event()

    def on_message(message, _data):
        if message.get("type") != "send":
            print(f"[agent] {message}", file=sys.stderr)
            return
        payload = message.get("payload") or {}
        records.append(payload)
        handle.write(json.dumps(payload) + "\n")
        handle.flush()
        kind = payload.get("type")
        if kind == "hook":
            ok = payload.get("ok")
            print(f"{'HOOK ' if ok else 'HOOK-FAILED '} {payload.get('name')} {payload.get('detail','')}")
        elif kind == "req-invade":
            print(
                f"REQ-INVADE  map={payload.get('ceremony_block')} "
                f"entry={payload.get('host_entry_point')} raw={payload.get('raw')}"
            )
        elif kind == "writer":
            print(
                f"WRITE       {payload.get('name')} = {payload.get('block') or payload.get('value')}"
                f"   <- {payload.get('caller_name')}"
            )
        elif kind == "placement":
            print(f"PLACEMENT   block={payload.get('block')} entry={payload.get('entry_point')}")

    script.on("message", on_message)
    script.on("destroyed", done.set)
    script.load()

    print(json.dumps(script.exports_sync.install(), indent=2))
    print(f"\nwriting {args.out}\nInvade someone, then Ctrl-C.\n")

    try:
        done.wait()
    except KeyboardInterrupt:
        pass
    finally:
        try:
            script.unload()
        except Exception:
            pass
        handle.close()

    verdict = classify(records)
    print("\n=== VERDICT ===")
    print(json.dumps(verdict, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
