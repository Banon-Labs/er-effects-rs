#!/usr/bin/env python3
"""Prove every repo STRUCT-field offset that a singleton reaches is still a field on 1.17.

Why this exists -- The one silent failure class
-----------------------------------------------
The 1.16.2 -> 1.17 migration has three ways to be wrong about an address, and only two of them
say anything:

  * a stale detour target        -> `er-hook` refuses the address and logs `HOOK REFUSED`;
  * an unmapped CALL/data RVA    -> the resolver returns 0 and the caller reports it;
  * a stale STRUCT field offset  -> `*(this + 0xNN)` returns the neighbouring field.

The third one has no refusal, no fault and no log line. It returns a plausible number of the
right width, forever. `crates/er-reload-trace/src/lib.rs` already records the shape in miniature:
the DLUID global moved 0x485dc18 -> 0x4861d28 and the read against the old slot still
"succeeded" -- it returned a byte from whatever now lives there.

Nothing in this repo checked field offsets against 1.17 until the 2026-08-30 audit, and the audit
found the cheapest possible oracle sitting unused.

The mechanism (no dataflow, no RTTI pairing, no heuristics)
-----------------------------------------------------------
Find `mov r64, [rip+disp32]` whose target is a known singleton global, then take any
`[reg + 0xNN]` in the next few instructions, before that register is written again. The base
register PROVABLY holds that singleton -- the instruction two lines up loaded it. So the
displacement is an object-level ground truth: it is a real field offset of whatever class lives
in that global, in that image, with no inference in between.

Run it on both flat de-Arxan'd images (file offset == RVA, base 0x140000000) and you get, per
object, the set of field offsets 1.16.2 reads and the set 1.17 reads. An offset the old image
reads and the new one never reads is a field that moved or vanished -- which is exactly the
silent failure, made loud.

Measured on the tracked objects today: Not one of the 312 field offsets 1.16.2 reads through
these globals has disappeared in 1.17 (GameMan 159, GameDataMan 53, CSMenuManImp 44,
WorldChrManImp 18, SessionManager 16, CSFlipperImp 7, PlayerGameData 15 -- every one of them read
in both images), and neither image reads one the other does not. So the null result is a real
null result, measured, not an empty set: see FROZEN_FIELD_COUNTS for why it cannot silently
become one.

This paragraph used to read "314 ... GameMan 160 ... SessionManager gains one field in 1.17", and
that extra field was not a field. `_follow` walked five instructions past the end of the function
holding the singleton load and collected whatever the next function did with the register; the
1.17 SessionManager "+0x18" was a `lea edx, [rax + 0x18]` six bytes over the boundary. See
`_follow` for the three artefacts and for why the correct bound is the function extent, never a
byte or instruction count.

What it still does not cover, said plainly. Only 65 of the workspace's ~900 named offset constants
are cleared here. 713 have no owner this can establish, 112 have an owner no singleton reaches
(CS::ChrIns, the dialog layouts, Scaleform::MemoryFile, the FileCap family), and 13 belong to a
scanned object but are never read through the singleton in either image -- including
PLAYER_GAME_DATA_IS_MAIN_PLAYER_OFFSET 0x8f0, which the constructor and vtable routes used for
the pins in crates/er-game-base/src/pgd.rs do witness. All of those are unknown. They are printed
as unknown, counted as unknown, and never rolled into the clean number.

One chain hop
-------------
`CS::PlayerGameData` is not in a global of its own -- it hangs off `GameDataMan + 0x8`
(`main_player_game_data`), which is the route 20+ live sites in this repo already take and the
one the sibling `fromsoftware-rs` binding declares. So a chain may carry hops: the scan follows
`mov r64,[rip+GameDataMan]; mov r64,[r64+0x8]` and then collects `[reg+0xNN]` off the result.
A hop is only admissible when the hop offset is itself witnessed on the parent object in both
images -- otherwise the hop is exactly the kind of unverified assumption this gate exists to
refuse -- and that admission is re-derived on every run, never asserted.

What this gate reports, and why it reports coverage
----------------------------------------------------
Nine "audits" in this repo have reported zero findings while real findings stood, because a
matcher that goes blind produces an empty set and `assert bad == 0` passes over an empty set.
So this prints three numbers, not a verdict:

  * how many repo constants it cleared (offset witnessed on its owner in both images);
  * how many it could not attribute (no owner, or an owner no singleton reaches) -- these are
    unknown, never "fine";
  * how many field offsets each scanned object contributed, per image.

The per-object field counts depend only on the two frozen images and this matcher, so they are
frozen exactly (`FROZEN_FIELD_COUNTS`): a change to the matcher that blinds it moves them and
this gate goes red instead of reporting a smaller clean set. The cleared-constant floor
(`MIN_CLEARED_CONSTANTS`) carries headroom instead, because it also depends on repo source that
lands continuously, and a ratchet that goes red when somebody deletes a constant is a ratchet
people learn to ignore.

Usage
  python3 scripts/check-singleton-field-offsets.py                  # the gate
  python3 scripts/check-singleton-field-offsets.py --census         # per-object field census
  python3 scripts/check-singleton-field-offsets.py --pgd-offsets    # PGD witnesses (three routes),
                                                                    # the evidence behind the
                                                                    # offset_of! pins in
                                                                    # crates/er-game-base/src/pgd.rs
  python3 scripts/check-singleton-field-offsets.py --selftest
  python3 scripts/check-singleton-field-offsets.py --prove-selftest-catches-regression

capstone is auto-provisioned via `uv run --with capstone` when it is not importable, the same way
scripts/dump-deobf-shift.py does it. The two de-Arxan'd images are gitignored (game-derived), so
a checkout without them SKIPs -- loudly, and never with the word OK.
"""

import argparse
import collections
import importlib.util
import io
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import function_extent  # noqa: E402 - repo-local, and the sys.path line above is what makes it work

# --- capstone bootstrap via uv (no persistent install needed) ------------------------------
# capstone is provisioned at runtime by `uv run --with capstone`; it is not in the base
# interpreter Pyright resolves against, so probe with find_spec (a bare import would be an
# unresolved-import error) and the `from capstone` imports below carry a documented ignore.
if importlib.util.find_spec("capstone") is None:
    if os.environ.get("_ER_SFO_BOOTSTRAPPED") != "1":
        os.environ["_ER_SFO_BOOTSTRAPPED"] = "1"
        os.execvp(  # noqa: S606 -- fixed argv, no shell
            "uv",
            ["uv", "run", "--with", "capstone", "python3", os.path.abspath(__file__)]
            + sys.argv[1:],
        )
    sys.exit("capstone unavailable and `uv run --with capstone` bootstrap failed")

from capstone import CS_ARCH_X86, CS_GRP_JUMP, CS_GRP_RET, CS_MODE_64, Cs  # pyright: ignore[reportMissingImports]
from capstone import x86 as cs_x86  # pyright: ignore[reportMissingImports]

BASE = 0x140000000
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OLD_IMAGE = os.path.join(REPO, "eldenring-deobf.bin")
NEW_IMAGE = os.path.join(REPO, "eldenring-deobf-1.17.bin")
DATA_MAP = os.path.join(REPO, "docs", "recon", "rva-map-1162-to-1170.data.tsv")

# How many instructions after the singleton load may still be reading through it. The
# register-clobber, branch and function-end stops in `_follow` are what actually bound the window;
# this is the belt. Swept against both images, with the function-end stop in place:
#
#   window |  fields witnessed per object          | asymmetric offsets (in one image only)
#   -------|---------------------------------------|---------------------------------------
#     3    | 151/52/44/16/16/7/10                   | none
#     5    | 159/53/44/18/16/7/15                   | none
#     8    | 161/55/45/18/16/7/16                   | none
#    12    | 161/55/45/18/16/7/17                   | none
#
# Read that last column against the one this table used to carry. Before the function-end stop
# landed (2026-08-31) the sweep reported `GameMan +0x0` lost at 3, and `CSMenuManImp +0x3b` /
# `SessionManager +0x73` gained at 8, and the tuning note blamed those on a branch join -- an
# address reached from elsewhere with a different value in the register. They were not joins.
# They were the next FUNCTION: every one of them decoded past a `ret`, out of the de-Arxan'd
# image's leftover gap bytes, where the "register" holds whatever its real owner put there.
# `GameMan +0x0`, the "vftable slot" that made 3 look too tight, is a phantom in both images --
# 1.17's two witnesses for it decode as `sar dword ptr [rax], 0x6f` and `and dword ptr [rax], esp`,
# neither of which was ever assembled. With the real bound in place nothing is asymmetric at any
# window, so this parameter is no longer load-bearing; 5 is kept because it is what the frozen
# counts were re-measured at, not because anything now hinges on it.
FOLLOW_INSNS = 5
# Widest plausible struct. Anything past this is an array stride or a mask, not a field.
FIELD_MAX = 0x20000

# ---------------------------------------------------------------------------------------------
# Which global holds which object.
#
# Every row is a claim, so each one is either measured by the 2026-08-30 struct-offset audit or
# reachable from the repo's own named route. A wrong row here would compare a constant against
# another class's field set, so this table is deliberately short: an object nobody can ground is
# left out, and its constants are then reported UNATTRIBUTED rather than waved through.
#
#   object -> (singleton constant in the tracked data map, hop offsets)
#
# A hop is only used when `hop_is_admissible` re-derives it from both images at run time.
# ---------------------------------------------------------------------------------------------
CHAINS = {
    # Measured all-identical by the audit's own singleton scan.
    "CS::GameMan": ("GAME_MAN_SINGLETON_RVA", ()),
    "CS::GameDataMan": ("GAME_DATA_MAN_GLOBAL_RVA", ()),
    "CS::CSMenuManImp": ("CS_MENU_MAN_GLOBAL_RVA", ()),
    "CS::WorldChrManImp": ("WORLD_CHR_MAN_GLOBAL_RVA", ()),
    "SessionManager": ("CS_SESSION_MANAGER_GLOBAL_RVA", ()),
    "CS::CSFlipperImp": ("CS_FLIPPER_SINGLETON_RVA", ()),
    # One hop. `GameDataMan + 0x8` is `main_player_game_data` in the sibling binding and the
    # route 20+ live sites in this workspace already take, spelled
    # GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET (crates/er-game-base/src/rva.rs).
    "CS::PlayerGameData": ("GAME_DATA_MAN_GLOBAL_RVA", (0x8,)),
}

# Repo constants name their owner by prefix far more often than any curated table can keep up
# with. These are the prefixes whose meaning is unambiguous; anything else falls through to the
# curated owners table and then to UNATTRIBUTED.
NAME_PREFIX_OWNERS = (
    ("PLAYER_GAME_DATA_", "CS::PlayerGameData"),
    ("PGD_", "CS::PlayerGameData"),
    ("GAME_DATA_MAN_", "CS::GameDataMan"),
    ("GAME_MAN_", "CS::GameMan"),
    ("GAMEMAN_", "CS::GameMan"),
    ("WORLD_CHR_MAN_", "CS::WorldChrManImp"),
    ("CS_MENU_MAN_", "CS::CSMenuManImp"),
    ("CSMENUMAN_", "CS::CSMenuManImp"),
    ("MENU_MAN_", "CS::CSMenuManImp"),
    ("SESSION_MANAGER_", "SessionManager"),
    ("CS_FLIPPER_", "CS::CSFlipperImp"),
    ("FLIPPER_", "CS::CSFlipperImp"),
)

# A constant whose name says it is an offset. Deliberately the audit's narrow A-tier: a name that
# merely contains offset somewhere is noise, and inline `+ 0xNN` cannot be told from an index.
OFFSET_NAME = re.compile(r"(_OFFSET(_[A-Z0-9]+)?$|_OFF$|_OFFS$|_FIELD$|_DISP$|_DISPLACEMENT$)")

# ---------------------------------------------------------------------------------------------
# Frozen coverage.
#
# The per-object counts below are (offsets witnessed in 1.16.2, in 1.17, in both). They are a
# function of the two frozen images and this matcher alone -- no repo source enters -- so they are
# pinned exactly. A change that blinds the scan (a dropped mov encoding, a narrowed follow window,
# a wrong global) moves them, and this gate goes red rather than reporting a smaller clean set.
# That is the whole point: `assert bad == 0` over an empty set is the failure this repo hit nine
# times in a week.
#
# Refresh deliberately with --refresh-frozen when the matcher is intentionally widened, and read
# the diff.
# ---------------------------------------------------------------------------------------------
# Re-measured 2026-08-31 with the function-end stop in `_follow`. Two rows moved, and both moved
# because a phantom left: GameMan 160 -> 159 (the `+0x0` "vftable slot", read past the `ret` in
# both images) and SessionManager (16, 17, 16) -> (16, 16, 16) (the 1.17 `+0x18` `lea`, six bytes
# into the next function). The gate's old headline -- "SessionManager gains one field in 1.17 and
# loses none" -- was an artefact of the over-read and is gone with it.
FROZEN_FIELD_COUNTS = {
    "CS::GameMan": (159, 159, 159),
    "CS::GameDataMan": (53, 53, 53),
    "CS::CSMenuManImp": (44, 44, 44),
    "CS::WorldChrManImp": (18, 18, 18),
    "SessionManager": (16, 16, 16),
    "CS::CSFlipperImp": (7, 7, 7),
    "CS::PlayerGameData": (15, 15, 15),
}

# Cleared repo constants. Unlike the counts above this one reads repo source, which lands
# continuously, so it is a floor with headroom rather than an exact pin -- a ratchet that goes red
# because somebody deleted a constant is a ratchet people route around. Measured today: 71.
MIN_CLEARED_CONSTANTS = 60


class Chain:
    """One resolved singleton route, with its per-image field-offset sets."""

    def __init__(self, obj, const, hops):
        self.obj = obj
        self.const = const
        self.hops = hops
        self.old_rva = None
        self.new_rva = None
        self.old = set()
        self.new = set()
        self.hop_ok = True
        self.hop_note = ""

    @property
    def both(self):
        return self.old & self.new

    @property
    def lost(self):
        return self.old - self.new

    @property
    def gained(self):
        return self.new - self.old


def read_singleton_map(path):
    """{constant: (rva_1162, rva_1170)} out of the tracked data map."""
    out = {}
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            if line.startswith("#") or not line.strip():
                continue
            fields = line.rstrip("\n").split("\t")
            if len(fields) < 3:
                continue
            out[fields[2]] = (int(fields[0], 16), int(fields[1], 16))
    return out


# `mov r64, [rip+disp32]`: REX.W (0x48) or REX.WR (0x4c), opcode 8B, modrm mod=00 rm=101.
# All 16 encodings, which is every way the compiler can load a qword global into a register.
RIP_MOV = re.compile(rb"[\x48\x4c]\x8b[\x05\x0d\x15\x1d\x25\x2d\x35\x3d]", re.S)

_MD = Cs(CS_ARCH_X86, CS_MODE_64)
_MD.detail = True


def _rip_mov_sites(image):
    """[(file_offset, dest_reg_index, target_rva)] for every `mov r64,[rip+d]` in `image`.

    Byte-matched first (a regex over 98 MB is milliseconds; decoding 98 MB is not), then each hit
    is confirmed by decoding it, so a coincidental three bytes inside another instruction cannot
    contribute a field offset.
    """
    out = []
    for match in RIP_MOV.finditer(image):
        off = match.start()
        rex, _, modrm = image[off : off + 3]
        disp = int.from_bytes(image[off + 3 : off + 7], "little", signed=True)
        target = (off + 7) + disp  # file offset == RVA, so this is already an RVA
        if not 0 <= target < len(image):
            continue
        reg = ((modrm >> 3) & 7) | (0x8 if rex & 0x4 else 0)
        out.append((off, reg, target))
    return out


# capstone register ids for the 16 general-purpose 64-bit registers, indexed the way the REX+modrm
# encoding numbers them.
_R64 = [
    cs_x86.X86_REG_RAX, cs_x86.X86_REG_RCX, cs_x86.X86_REG_RDX, cs_x86.X86_REG_RBX,
    cs_x86.X86_REG_RSP, cs_x86.X86_REG_RBP, cs_x86.X86_REG_RSI, cs_x86.X86_REG_RDI,
    cs_x86.X86_REG_R8, cs_x86.X86_REG_R9, cs_x86.X86_REG_R10, cs_x86.X86_REG_R11,
    cs_x86.X86_REG_R12, cs_x86.X86_REG_R13, cs_x86.X86_REG_R14, cs_x86.X86_REG_R15,
]
_R64_INDEX = {reg: i for i, reg in enumerate(_R64)}


def _follow(image, off, reg_index, hops):
    """Field offsets read through the register loaded at `off`, after walking `hops`.

    Returns a set of displacements. The window closes at the first of: The end of the function
    the load is in, FOLLOW_INSNS instructions, a write to the register being tracked (after which
    it no longer holds the singleton), a branch, or a decode failure.

    The function end is the one that was missing, and it invented fields. The premise of this
    whole gate -- "the base register PROVABLY holds that singleton, the instruction two lines up
    loaded it" -- is a statement about straight-line code inside one function. Past the `ret` it
    is simply false: control does not fall through, the register belongs to whoever comes next,
    and in a de-Arxan'd image the bytes there are the deobfuscator's leftovers, which
    RESYNCHRONISE into instructions nobody assembled. Three measured artefacts, all removed by
    the `body_end` stop below (2026-08-31):

      * SessionManager 1.17 `+0x18`. The load sits at 0x140257d0f inside a function ending at
        0x140257d22; the "read" is `lea edx, [rax + 0x18]` at 0x140257d28, six bytes into the
        next function -- and a `lea` computing an address, not a field read, at that. It was the
        sole evidence for this file's headline claim that SessionManager gains a field in 1.17.
      * CS::GameMan `+0x0`, in both images, from two sites each, every one past its function's
        end. 1.17's decode as `sar dword ptr [rax], 0x6f` and `and dword ptr [rax], esp`. This
        is the "vftable slot" the FOLLOW_INSNS table above used to cite as the reason a window of
        3 was too tight.
      * The `+0x3b` / `+0x73` junk the same table blamed on a branch join at window 8. Also the
        next function.

    Being symmetric is no defence: `CS::PlayerGameData +0xe5` reads past the first `.pdata` chunk
    in both images and looks clean because both images agree. It survives here only because
    `function_regions` MERGES chunk runs, so the read is genuinely inside the function -- which is
    exactly why the extent comes from the shared primitive and not from a local `.pdata` walk. A
    hand-rolled reader written for this fix dropped it, wrongly.
    """
    want = _R64[reg_index]
    found = set()
    remaining = list(hops)
    pos = off + 7
    seen = 0
    # None means the extent cannot be told at all. There is no honest byte count to substitute,
    # so the other stops (register clobber, branch, FOLLOW_INSNS) carry it, exactly as before --
    # this arm is not a licence to over-read, it is the case where nothing better is known.
    # Measured on the current images: 2 of the follow sites.
    end = function_extent.body_end(image, BASE + off)
    for insn in _MD.disasm(bytes(image[pos : pos + 64]), BASE + pos):
        if seen >= FOLLOW_INSNS:
            break
        if end is not None and insn.address - BASE + insn.size > end:
            break
        seen += 1
        hop_taken = False
        for operand in insn.operands:
            if operand.type != cs_x86.X86_OP_MEM:
                continue
            mem = operand.mem
            if mem.base != want or mem.index != 0 or mem.segment != 0:
                continue
            if not 0 <= mem.disp <= FIELD_MAX:
                continue
            if remaining:
                # Still walking to the object: this displacement must be the hop, and the
                # instruction must be the load that takes us there.
                if mem.disp == remaining[0] and insn.id == cs_x86.X86_INS_MOV:
                    dest = insn.operands[0]
                    if dest.type == cs_x86.X86_OP_REG and dest.reg in _R64_INDEX:
                        want = dest.reg
                        remaining.pop(0)
                        seen = 0
                        hop_taken = True
                        break
            else:
                found.add(mem.disp)
        if hop_taken:
            continue
        # The register stops holding the singleton the moment anything writes it.
        _, written = insn.regs_access()
        if any(_MD.reg_name(r) == _MD.reg_name(want) for r in written):
            break
        if insn.group(CS_GRP_JUMP):
            break
    return found


def scan(image, targets, hops_by_target):
    """{target_rva: {offset, ...}} for every requested singleton RVA in one image."""
    out = {rva: set() for rva in targets}
    for off, reg, target in _rip_mov_sites(image):
        if target not in out:
            continue
        out[target] |= _follow(image, off, reg, hops_by_target[target])
    return out


def build_chains(old_image, new_image, singletons, chains=CHAINS):
    """Resolve chains against both images. Returns ([Chain], [problem strings])."""
    problems = []
    resolved = []
    for obj, (const, hops) in chains.items():
        chain = Chain(obj, const, hops)
        if const not in singletons:
            problems.append(f"{obj}: {const} is not in {os.path.relpath(DATA_MAP, REPO)}")
            continue
        chain.old_rva, chain.new_rva = singletons[const]
        resolved.append(chain)

    # Pass 1: the hopless chains, which is also what the hop admission test reads.
    flat = [c for c in resolved if not c.hops]
    for image, attr, key in ((old_image, "old", 0), (new_image, "new", 1)):
        want = {c.old_rva if key == 0 else c.new_rva: () for c in flat}
        sets = scan(image, set(want), want)
        for chain in flat:
            rva = chain.old_rva if key == 0 else chain.new_rva
            setattr(chain, attr, sets[rva])

    # Pass 2: the chains with hops. A hop is admissible only when the hop offset is itself
    # witnessed on the parent object in both images -- re-derived here, never asserted.
    hopped = [c for c in resolved if c.hops]
    for chain in hopped:
        parent = next((c for c in flat if c.const == chain.const), None)
        if parent is None:
            chain.hop_ok = False
            chain.hop_note = f"no hopless chain on {chain.const} to admit the hop against"
            problems.append(f"{chain.obj}: {chain.hop_note}")
            continue
        first = chain.hops[0]
        if first not in parent.old or first not in parent.new:
            chain.hop_ok = False
            chain.hop_note = (
                f"hop +{first:#x} is not witnessed on {parent.obj} in both images "
                f"(1.16.2 {first in parent.old}, 1.17 {first in parent.new})"
            )
            problems.append(f"{chain.obj}: {chain.hop_note}")
            continue
        chain.hop_note = f"via {parent.obj} +{first:#x} (witnessed in both images)"
    good = [c for c in hopped if c.hop_ok]
    if good:
        for image, attr, key in ((old_image, "old", 0), (new_image, "new", 1)):
            want = {}
            for chain in good:
                want[chain.old_rva if key == 0 else chain.new_rva] = chain.hops
            sets = scan(image, set(want), want)
            for chain in good:
                rva = chain.old_rva if key == 0 else chain.new_rva
                setattr(chain, attr, sets[rva])

    order = list(chains)
    resolved.sort(key=lambda c: order.index(c.obj))
    return resolved, problems


# ---------------------------------------------------------------------------------------------
# Repo side.
# ---------------------------------------------------------------------------------------------
def _curated_owners():
    """The hand-read constant -> class table from scripts/adjudicate-autoload-offsets.py.

    Imported rather than copied: a copied ownership table is a second claim about the same fact
    that drifts silently, which is the defect this repo's own `EXHAUSTIVE_VERDICTS` copy had.
    A failure to import is a hard error, not a shrug -- a gate that quietly loses its ownership
    table reports fewer constants and still says OK.
    """
    path = os.path.join(REPO, "scripts", "adjudicate-autoload-offsets.py")
    spec = importlib.util.spec_from_file_location("_er_sfo_adjudicate", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return {
        name: owner
        for name, owner in module.OWNERS.items()
        if isinstance(owner, str) and not owner.startswith("@")
    }


def owner_of(name, curated):
    short = name.split("::")[-1]
    if short in curated:
        return curated[short]
    for prefix, obj in NAME_PREFIX_OWNERS:
        if short.startswith(prefix):
            return obj
    return None


def repo_offsets():
    """[(constant, value, file, line)] for every named offset constant that resolves to a number."""
    sys.path.insert(0, os.path.join(REPO, "scripts"))
    import rva_symbols  # noqa: PLC0415 -- path is set up immediately above

    index = rva_symbols.index()
    rows = []
    for decl in index.decls:
        if not OFFSET_NAME.search(decl.symbol):
            continue
        values = decl.value or []
        for value in sorted(v for v in values if isinstance(v, int) and 0 <= v <= FIELD_MAX):
            rows.append((decl.qualified, value, os.path.relpath(decl.path, REPO), decl.line))
    return rows


def classify(rows, chains, curated):
    """Sort every repo offset row into cleared / lost / not-witnessed / UNATTRIBUTED."""
    by_obj = {c.obj: c for c in chains}
    out = collections.defaultdict(list)
    for name, value, path, line in rows:
        obj = owner_of(name, curated)
        if obj is None:
            out["UNATTRIBUTED-NO-OWNER"].append((name, value, obj, path, line))
            continue
        chain = by_obj.get(obj)
        if chain is None or not chain.hop_ok:
            out["UNATTRIBUTED-NO-CHAIN"].append((name, value, obj, path, line))
            continue
        if value in chain.old and value in chain.new:
            out["CLEARED"].append((name, value, obj, path, line))
        elif value in chain.old:
            out["LOST"].append((name, value, obj, path, line))
        else:
            out["NOT-WITNESSED"].append((name, value, obj, path, line))
    return out


# ---------------------------------------------------------------------------------------------
# PGD evidence mode (--pgd-offsets). Not part of the gate: nothing here feeds a verdict or a
# frozen count. It exists so the offsets pinned in crates/er-game-base/src/pgd.rs can be
# RE-derived, instead of resting on a one-off script somebody ran once and deleted.
#
# Three routes, each proving the base register holds a PlayerGameData without any cross-image
# function pairing. The sets are built per image and then intersected, so both means 1.16.2 code
# and 1.17 code each read that offset as a PGD field:
#
#   A  the singleton chain, GameDataMan(global) -> +0x8 -> PGD  (the gate's own route)
#   B  functions called with that chain's PGD pointer still in an argument register, one deep
#   C  the CS::PlayerGameData constructor, the functions it calls with `this` in RCX, and the
#      methods of the two vtables it stores at [this+0]
#
# The constructor pair below was produced by scripts/map-rvas-1162-to-1170.py and is checked here
# rather than trusted: each address must store both declared vtable pointers at [this+0], or this
# mode refuses to report. A drifted constructor address then says so instead of quietly scanning
# an unrelated function and printing a confident, wrong witness set.
# ---------------------------------------------------------------------------------------------
PGD_CTOR = {"old": 0x14025D580, "new": 0x14025D550}
PGD_VTABLES = {"old": (0x1429E15F8, 0x1429E5FA8), "new": (0x1429E45F8, 0x1429E8FA8)}
ARG_REGS = (cs_x86.X86_REG_RCX, cs_x86.X86_REG_RDX, cs_x86.X86_REG_R8, cs_x86.X86_REG_R9)
VOLATILE = {
    cs_x86.X86_REG_RAX, cs_x86.X86_REG_RCX, cs_x86.X86_REG_RDX, cs_x86.X86_REG_R8,
    cs_x86.X86_REG_R9, cs_x86.X86_REG_R10, cs_x86.X86_REG_R11,
}
TEXT_LO, TEXT_HI = 0x140001000, 0x142900000
FUNC_SCAN_BYTES = 0x3000


def _walk_function(image, va, arg_reg=cs_x86.X86_REG_RCX):
    """Field offsets read through `arg_reg`, plus the vtable pointers stored at [reg+0].

    Bounded by the function's own extent, with FUNC_SCAN_BYTES surviving only as a cap on top of
    it. The `ret` stop below is not a substitute: a function that ends in a tail call never
    reaches one, and the walk then ran up to FUNC_SCAN_BYTES into its neighbours. See `_follow`
    for the three field offsets that shape invented in the gate proper.
    """
    held = {arg_reg}
    fields, calls, stored = set(), [], []
    lea_targets = {}
    off = va - BASE
    stop = function_extent.body_slice_end(image, va, cap=FUNC_SCAN_BYTES)
    if stop is None:
        stop = off + FUNC_SCAN_BYTES
    for insn in _MD.disasm(bytes(image[off:stop]), va):
        if (
            insn.id == cs_x86.X86_INS_MOV
            and len(insn.operands) == 2
            and insn.operands[0].type == cs_x86.X86_OP_REG
            and insn.operands[1].type == cs_x86.X86_OP_REG
            and insn.operands[1].reg in held
            and insn.operands[0].reg in _R64_INDEX
        ):
            held.add(insn.operands[0].reg)
            continue
        if (
            insn.id == cs_x86.X86_INS_LEA
            and insn.operands[1].type == cs_x86.X86_OP_MEM
            and insn.operands[1].mem.base == cs_x86.X86_REG_RIP
        ):
            lea_targets[insn.operands[0].reg] = insn.address + insn.size + insn.operands[1].mem.disp
        for operand in insn.operands:
            if operand.type != cs_x86.X86_OP_MEM:
                continue
            mem = operand.mem
            if mem.base in held and mem.index == 0 and 0 <= mem.disp <= FIELD_MAX:
                fields.add(mem.disp)
                if (
                    mem.disp == 0
                    and insn.id == cs_x86.X86_INS_MOV
                    and insn.operands[1].type == cs_x86.X86_OP_REG
                    and insn.operands[1].reg in lea_targets
                ):
                    stored.append(lea_targets[insn.operands[1].reg])
        if insn.id == cs_x86.X86_INS_CALL:
            if (
                cs_x86.X86_REG_RCX in held
                and insn.operands
                and insn.operands[0].type == cs_x86.X86_OP_IMM
            ):
                calls.append(insn.operands[0].imm)
            held -= VOLATILE
            continue
        if insn.operands and insn.operands[0].type == cs_x86.X86_OP_REG and insn.operands[0].reg in held:
            same = (
                insn.id == cs_x86.X86_INS_MOV
                and len(insn.operands) == 2
                and insn.operands[1].type == cs_x86.X86_OP_REG
                and insn.operands[1].reg in held
            )
            if not same:
                held.discard(insn.operands[0].reg)
        if insn.group(CS_GRP_RET):
            break
    return fields, calls, stored


def _chain_call_targets(image, global_rva, hop, follow=12):
    """(fields, {(callee, arg_reg)}) for the PGD the chain produces, over the whole image."""
    fields, targets = set(), set()
    for off, reg_index, target in _rip_mov_sites(image):
        if target != global_rva:
            continue
        want, hopped, seen = _R64[reg_index], False, 0
        # Same function-end stop as `_follow`, for the same reason: a chain hop read out of the
        # next function is not this singleton's field.
        end = function_extent.body_end(image, BASE + off)
        for insn in _MD.disasm(bytes(image[off + 7 : off + 7 + 128]), BASE + off + 7):
            if seen >= follow:
                break
            if end is not None and insn.address - BASE + insn.size > end:
                break
            seen += 1
            took = False
            for operand in insn.operands:
                if operand.type != cs_x86.X86_OP_MEM:
                    continue
                mem = operand.mem
                if mem.base != want or mem.index != 0:
                    continue
                if not hopped:
                    if (
                        mem.disp == hop
                        and insn.id == cs_x86.X86_INS_MOV
                        and insn.operands[0].type == cs_x86.X86_OP_REG
                    ):
                        want, hopped, seen, took = insn.operands[0].reg, True, 0, True
                        break
                elif 0 <= mem.disp <= FIELD_MAX:
                    fields.add(mem.disp)
            if took:
                continue
            if (
                hopped
                and insn.id == cs_x86.X86_INS_CALL
                and want in ARG_REGS
                and insn.operands
                and insn.operands[0].type == cs_x86.X86_OP_IMM
            ):
                targets.add((insn.operands[0].imm, want))
                break
            _, written = insn.regs_access()
            if any(_MD.reg_name(r) == _MD.reg_name(want) for r in written):
                break
    return fields, targets


def _vtable_methods(image, vtable_va, limit=64):
    off = vtable_va - BASE
    out = []
    for i in range(limit):
        word = int.from_bytes(image[off + i * 8 : off + i * 8 + 8], "little")
        if not TEXT_LO <= word < TEXT_HI:
            break
        out.append(word)
    return out


def _pgd_fields(image, key, global_rva):
    fields, targets = _chain_call_targets(image, global_rva, 0x8)
    routes = {"A-chain-direct": len(fields), "B-callees": len(targets)}
    for callee, arg_reg in sorted(targets):
        if TEXT_LO <= callee < TEXT_HI:
            more, _, _ = _walk_function(image, callee, arg_reg=arg_reg)
            fields |= more
    ctor_fields, ctor_calls, stored = _walk_function(image, PGD_CTOR[key])
    declared = set(PGD_VTABLES[key])
    if not declared.issubset(set(stored)):
        return None, routes, (
            f"the constructor at {PGD_CTOR[key]:#x} does not store "
            + " and ".join(f"{v:#x}" for v in sorted(declared))
            + f" at [this+0] (it stores {sorted(hex(s) for s in set(stored))}); the address or the "
            "vtables drifted, so route C is refusing to report rather than scan the wrong function"
        )
    fields |= ctor_fields
    for callee in ctor_calls:
        if TEXT_LO <= callee < TEXT_HI:
            more, _, _ = _walk_function(image, callee)
            fields |= more
    for vtable in PGD_VTABLES[key]:
        for method in _vtable_methods(image, vtable):
            more, _, _ = _walk_function(image, method)
            fields |= more
    routes["C-ctor+vtable"] = len(ctor_calls)
    return fields, routes, None


def pgd_evidence(old_image, new_image, chains):
    """Print every CS::PlayerGameData field offset each image reads, and where they diverge."""
    chain = next(c for c in chains if c.obj == "CS::PlayerGameData")
    old_fields, old_routes, old_err = _pgd_fields(old_image, "old", chain.old_rva)
    new_fields, new_routes, new_err = _pgd_fields(new_image, "new", chain.new_rva)
    for err in (old_err, new_err):
        if err:
            print(f"FAIL: {err}")
            return 1
    both = old_fields & new_fields
    print(f"routes 1.16.2 {old_routes}   1.17 {new_routes}")
    print(f"1.16.2 reads {len(old_fields)} PGD offsets, 1.17 reads {len(new_fields)}, {len(both)} in both")
    diverge = sorted((old_fields ^ new_fields))
    if diverge:
        print(f"first offset the two images disagree about: {diverge[0]:#x}")
    for offset in sorted(old_fields | new_fields):
        where = "BOTH" if offset in both else ("1.16.2-ONLY" if offset in old_fields else "1.17-ONLY")
        gate = "  (gate route A)" if offset in chain.both else ""
        print(f"{offset:#06x}\t{where}{gate}")
    return 0


def load(path):
    with open(path, "rb") as handle:
        return handle.read()


def report(chains, buckets, problems, out=sys.stdout):
    print("objects scanned (field offsets witnessed through the singleton):", file=out)
    for chain in chains:
        note = f"  [{chain.hop_note}]" if chain.hop_note else ""
        if not chain.hop_ok:
            print(f"  {chain.obj:24s} UNUSABLE{note}", file=out)
            continue
        print(
            f"  {chain.obj:24s} 1.16.2 {len(chain.old):4d}   1.17 {len(chain.new):4d}   "
            f"identical {len(chain.both):4d}   lost {len(chain.lost):3d}   "
            f"new {len(chain.gained):3d}{note}",
            file=out,
        )
    cleared = buckets["CLEARED"]
    print(file=out)
    print("repo offset constants:", file=out)
    print(f"  CLEARED               {len(cleared):5d}  (witnessed on its owner in BOTH images)", file=out)
    print(f"  LOST                  {len(buckets['LOST']):5d}  (witnessed in 1.16.2, GONE in 1.17)", file=out)
    print(
        f"  NOT-WITNESSED         {len(buckets['NOT-WITNESSED']):5d}  "
        "(owner is scanned, offset never read through the singleton -- UNKNOWN, not safe)",
        file=out,
    )
    print(
        f"  UNATTRIBUTED-NO-CHAIN {len(buckets['UNATTRIBUTED-NO-CHAIN']):5d}  "
        "(owner known, no singleton reaches it -- UNKNOWN)",
        file=out,
    )
    print(
        f"  UNATTRIBUTED-NO-OWNER {len(buckets['UNATTRIBUTED-NO-OWNER']):5d}  "
        "(no owner could be established -- UNKNOWN)",
        file=out,
    )
    per_obj = collections.Counter(row[2] for row in cleared)
    if per_obj:
        print("  cleared by owner: " + ", ".join(f"{k} {v}" for k, v in sorted(per_obj.items())), file=out)
    # Name the not-witnessed rows. They are the ones a reader can act on: the owner is scanned, so
    # the offset simply never appears in either image through this route, and calling that "clean"
    # is the exact move this gate refuses to make on anyone's behalf.
    for name, value, obj, path, line in sorted(buckets["NOT-WITNESSED"]):
        print(f"    NOT-WITNESSED  {name} = {value:#x}  on {obj}  ({path}:{line})", file=out)
    for problem in problems:
        print(f"  PROBLEM: {problem}", file=out)


def check_frozen(chains, buckets, out=sys.stdout):
    """The coverage assertions. A drop must be as visible as a mismatch."""
    failures = []
    seen = {c.obj: c for c in chains}
    for obj, (old_n, new_n, both_n) in sorted(FROZEN_FIELD_COUNTS.items()):
        chain = seen.get(obj)
        if chain is None or not chain.hop_ok:
            failures.append(f"{obj}: frozen at {old_n}/{new_n}/{both_n} but the chain did not resolve")
            continue
        got = (len(chain.old), len(chain.new), len(chain.both))
        if got != (old_n, new_n, both_n):
            failures.append(
                f"{obj}: field counts {got} != frozen {(old_n, new_n, both_n)} -- the matcher "
                "changed what it can see; re-read the diff before refreshing"
            )
    extra = sorted(set(seen) - set(FROZEN_FIELD_COUNTS))
    if extra:
        failures.append(f"objects scanned but not frozen: {', '.join(extra)}")
    cleared = len(buckets["CLEARED"])
    if cleared < MIN_CLEARED_CONSTANTS:
        failures.append(
            f"cleared {cleared} constants, floor is {MIN_CLEARED_CONSTANTS} -- coverage DROPPED; "
            "this is a finding, not a smaller clean run"
        )
    for name, value, obj, path, line in sorted(buckets["LOST"]):
        failures.append(
            f"{name} = {value:#x} on {obj} is read in 1.16.2 and NEVER in 1.17 "
            f"({path}:{line}) -- the field moved or vanished"
        )
    for line in failures:
        print(f"FAIL: {line}", file=out)
    return 1 if failures else 0


def selftest(old_image, new_image, singletons, out=None):
    """Positive control, coverage floors, and a negative control -- so green is not vacuous."""
    chains, problems = build_chains(old_image, new_image, singletons)
    curated = _curated_owners()
    buckets = classify(repo_offsets(), chains, curated)
    out = sys.stdout if out is None else out
    # The full coverage report belongs to the live run, which check.sh prints. Here it is captured
    # and replayed only when something is wrong, so the selftest does not double every number.
    captured = io.StringIO()
    report(chains, buckets, problems, out=captured)
    rc = check_frozen(chains, buckets, out=captured)
    print(
        f"selftest: {sum(len(c.both) for c in chains if c.hop_ok)} field offsets witnessed in both "
        f"images across {sum(1 for c in chains if c.hop_ok)} objects, "
        f"{len(buckets['CLEARED'])} repo constants cleared",
        file=out,
    )
    if rc:
        print(captured.getvalue(), file=out, end="")
        print("selftest FAIL: the live check is red, so its controls prove nothing", file=out)
        return 1
    if problems:
        print(f"selftest FAIL: {len(problems)} unresolved chain problem(s)", file=out)
        return 1

    # Negative control 1 -- a constant on a moved field must be caught. Take an offset each
    # object reads in 1.16.2 and never in 1.17 if one exists; otherwise fabricate one by
    # taking a witnessed 1.16.2 offset and deleting it from the 1.17 set.
    caught = 0
    planted = 0
    for chain in chains:
        if not chain.hop_ok or not chain.both:
            continue
        victim = sorted(chain.both)[len(chain.both) // 2]
        shadow = Chain(chain.obj, chain.const, chain.hops)
        shadow.old, shadow.new = set(chain.old), set(chain.new) - {victim}
        others = [c for c in chains if c is not chain]
        rows = [(f"{chain.obj.split('::')[-1]}_PLANTED_OFFSET", victim, "synthetic", 0)]
        fake = classify(rows, others + [shadow], {f"{chain.obj.split('::')[-1]}_PLANTED_OFFSET": chain.obj})
        planted += 1
        caught += len(fake["LOST"])
    if planted == 0:
        print("selftest FAIL: could not plant a control -- nothing was witnessed", file=out)
        return 1
    if caught != planted:
        print(f"selftest FAIL: planted {planted} moved fields, caught {caught}", file=out)
        return 1
    print(f"  negative control: {caught}/{planted} planted moved fields rejected", file=out)

    # Negative control 2 -- the hop admission must actually gate. A hop offset that is not
    # witnessed on the parent has to make the chain UNUSABLE, not silently scan garbage.
    bogus = dict(CHAINS)
    bogus["CS::PlayerGameData"] = ("GAME_DATA_MAN_GLOBAL_RVA", (0x7,))
    _, hop_problems = build_chains(old_image, new_image, singletons, chains=bogus)
    if not any("hop +0x7" in p for p in hop_problems):
        print("selftest FAIL: an unwitnessed hop offset was admitted", file=out)
        return 1
    print("  hop control: an unwitnessed hop (+0x7) is refused", file=out)
    print("selftest OK", file=out)
    return 0


def prove_selftest_catches_regression(old_image, new_image, singletons):
    """Blind the matcher on purpose; --selftest must go red.

    A green selftest only means something if a broken instrument would fail it. This narrows the
    follow window to zero instructions -- the single most likely way for this scan to quietly stop
    seeing fields -- and requires the whole selftest to fail.
    """
    global FOLLOW_INSNS
    real = FOLLOW_INSNS
    FOLLOW_INSNS = 0
    seen = io.StringIO()
    try:
        code = selftest(old_image, new_image, singletons, out=seen)
    finally:
        FOLLOW_INSNS = real
    lines = [line for line in seen.getvalue().splitlines() if line.startswith(("FAIL", "selftest FAIL"))]
    if code == 0:
        print("FAIL: the selftest passed with a blinded matcher -- it is vacuous")
        return 1
    print("regression proof OK: FOLLOW_INSNS=0 (a matcher that sees no fields) fails the selftest.")
    print("  what the blinded run reported:")
    for line in lines[:6]:
        print(f"    {line}")
    if len(lines) > 6:
        print(f"    ... and {len(lines) - 6} more")
    return 0


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--old", default=OLD_IMAGE)
    parser.add_argument("--new", default=NEW_IMAGE)
    parser.add_argument("--map", default=DATA_MAP)
    parser.add_argument("--census", action="store_true", help="print every witnessed offset per object")
    parser.add_argument(
        "--pgd-offsets",
        action="store_true",
        help="print the CS::PlayerGameData witnesses (the evidence behind the offset_of! pins)",
    )
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument(
        "--prove-selftest-catches-regression",
        action="store_true",
        help="blind the matcher on purpose; the selftest must go red",
    )
    parser.add_argument(
        "--refresh-frozen", action="store_true", help="print FROZEN_FIELD_COUNTS for this matcher"
    )
    args = parser.parse_args()

    # The two de-Arxan'd images are gitignored (game-derived bytes are never committed), so a
    # fresh checkout and CI do not have them. Skip at exit 0 -- but say skipped, name the missing
    # file, and never print the word this gate prints when it passes. A gate that cannot run must
    # not read like a gate that ran.
    for path in (args.old, args.new, args.map):
        if not os.path.isfile(path):
            print(f"SKIPPED (NOT A PASS): {path} is absent, so no field offset was checked at all")
            print("  the two de-Arxan'd images are gitignored; run this on a machine that has them")
            return 0

    singletons = read_singleton_map(args.map)
    old_image = load(args.old)
    new_image = load(args.new)

    if args.prove_selftest_catches_regression:
        return prove_selftest_catches_regression(old_image, new_image, singletons)
    if args.selftest:
        return selftest(old_image, new_image, singletons)

    chains, problems = build_chains(old_image, new_image, singletons)

    if args.refresh_frozen:
        print("FROZEN_FIELD_COUNTS = {")
        for chain in chains:
            print(f'    "{chain.obj}": ({len(chain.old)}, {len(chain.new)}, {len(chain.both)}),')
        print("}")
        return 0

    if args.census:
        for chain in chains:
            print(f"== {chain.obj}  ({chain.const})")
            print("   1.16.2:", " ".join(f"{o:#x}" for o in sorted(chain.old)))
            print("   1.17  :", " ".join(f"{o:#x}" for o in sorted(chain.new)))
            if chain.lost:
                print("   LOST  :", " ".join(f"{o:#x}" for o in sorted(chain.lost)))
            if chain.gained:
                print("   NEW   :", " ".join(f"{o:#x}" for o in sorted(chain.gained)))
        return 0

    if args.pgd_offsets:
        return pgd_evidence(old_image, new_image, chains)

    curated = _curated_owners()
    buckets = classify(repo_offsets(), chains, curated)
    report(chains, buckets, problems)
    return check_frozen(chains, buckets)


if __name__ == "__main__":
    sys.exit(main())
