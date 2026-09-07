#!/usr/bin/env python3
"""Generate the shipped `er-npc-possess` moveset table from the unpacked corpus.

The table is integers only -- chr id, animation ids, bucket, rank, reach band and a
denial-reason code. No game bytes, no asset payloads, no strings lifted out of the
game: everything here is a decision *about* an integer id, and the ids themselves are
already public knowledge (they are BND entry filenames). That is what makes it
committable under the repo's no-game-derived-binaries rule.

Five offline sources, joined per creature:

  behavior graph   <chr>.behbnd.dcx (Havok TAG0)  fireable event name -> state -> clip
  TimeAct          <chr>.tae                      ability events + event times
  regulation.bin   NpcParam -> BehaviorParam      judge id -> AtkParam_Npc / Bullet
  regulation.bin   AtkParam_Npc                   damage, hit-capsule radii, throwTypeId
  regulation.bin   ThrowParam                     throwTypeId -> victim chr id + range

A grab is an ordinary attack with one column set. It is not a 4000-band animation and it
is not TAE event 304. `CS::ChrDamageModule::ApplyDamage` looks up the landed hit's
`AtkParam` row, reads `throwTypeId`, and -- before any damage is calculated -- calls
`CSChrThrowModule::InitThrow(attackerThrowModule, victimChrIns, throwTypeId)`.
`CSThrowNode::ValidateAttemptAndReturnParamId` then scans `ThrowParam` for a row matching
(attacker npcId, victim npcId, throwTypeId); on a hit, the throw system drives both parties
into that row's `atkAnimId`/`defAnimId`. Those are the 4000-band clips, which is exactly why
no event name reaches them: they are downstream of a throw that has already been accepted.

So the fireable thing is the INITIATOR, and it was already in this table as a plain attack.
Swept over the corpus: 153 fireable initiator animations across 78 creatures (169 counting
(animation, throwTypeId) pairs), every one in the 3000 band, against 108 TAE-304 clips of
which zero are fireable.

Why the graph and not the TAE decides what SHIPS: the graph is a strict superset (c2120
has 41 fireable attack events against 36 TAE attacks, c3200 30 against 10) and every TAE
attack animation has a graph event. So the graph says what can be fired and the TAE says
which of those opens a damage window.

FIREABILITY is the whole point of this script. Declared != fireable: median 1366 declared
event names per chr against median 580 that a `hkbStateMachine::TransitionInfo` actually
consumes. Firing a declared-but-unconsumed name resolves fine and then silently no-OPS --
no error, no log, nothing on screen -- so the gate has to happen here, at generation time,
where it can be checked, rather than at runtime where it cannot be observed.

Usage:
  scripts/er-moveset-table-gen.py --out crates/er-npc-possess/data/moveset.tbl
  scripts/er-moveset-table-gen.py --event-histogram          # TAE types seen in-band
  scripts/er-moveset-table-gen.py --only c4500,c2120 --out -
"""
import argparse
import collections
import contextlib
import glob
import importlib.util
import multiprocessing
import os
import re
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_CORPUS = ('/home/banon/er-extract/LOOK_HERE_WITCHY_RECURSIVE_20260713/'
                  'sharded/chr')
CORPUS_ROOT = os.environ.get('ER_CHR_CORPUS_ROOT', DEFAULT_CORPUS)


def _load(name, filename):
    spec = importlib.util.spec_from_file_location(name, os.path.join(HERE, filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


BMAP = _load('bmap', 'er-behbnd-attack-map.py')
TAE = _load('taescan', 'er-tae-event-scan.py')
PR = _load('paramread', 'er-param-read.py')

#: Table format version. Bump when the row grammar changes; the Rust parser refuses
#: a version it was not written for rather than misreading columns.
TABLE_VERSION = 4

#: The animation-id bands this table covers. Banding is a FromSoft convention, not a
#: proof -- the authoritative "is this an attack" answer is the BehaviorParam join
#: below -- but it is consistent across every creature chr checked, and it is what
#: keeps the table at a few thousand rows instead of thirty thousand.
ATTACK_BAND = (3000, 3999)
GRAB_BAND = (4000, 4999)
STEP_BAND = (6000, 6023)
BANDS = (ATTACK_BAND, GRAB_BAND, STEP_BAND)

#: `CSChrEventModule::requestAnimationId` formats its event name as `W_Event%04d`
#: unconditionally. That field write costs no game function address, so it is always the
#: Preferred way to fire an animation -- but it is not a universal one.
FIRE_PREFIX = 'W_Event'

#: Every spelling an animation ID can have, in the order the generator prefers them.
#:
#: `W_Event` is a broad alias layer, not a total one: measured at 88.4% num==anim-id
#: across the corpus, against 100% for `W_Step` in 6000-6023 and for the ride families.
#: So dodges, goal actions and the ride sets have no `W_Event` name, and the field write
#: cannot ask for them however fireable they are -- c2120's 6000/6001/6002/6003/6011
#: resolve under `W_Step` and under nothing else.
#:
#: The order is the policy. `W_Event` first, because choosing it keeps the possession on
#: the path that resolves no game function address. Everything after it costs one call to
#: `PlayAnimationByBehaviorName`, so it is only chosen when `W_Event` cannot reach the id.
#: The ride families sit last on purpose: `W_Ridden_Enemy_Step` is what a mount plays
#: while someone is riding it, and driving that into a free-standing creature is the kind
#: of thing the runtime watchdog exists to catch. They are kept because they are 100%
#: identity and are sometimes the only spelling that reaches an id at all.
#:
#: The prefix is resolved per CHR from that CHR'S own event table, never from the band.
#: A band tells you what to try; only the chr's own `TransitionInfo` set says what works.
PREFIXES = (
    'W_Event',
    'W_Attack',
    'W_Step',
    'W_GuardAttack',
    'W_GoalAction',
)

#: The ride families, scanned and then dropped -- measured, not assumed.
#:
#: `W_Ride_Attack_`, `W_RideStep`, `W_Ridden_Enemy_Attack`, `W_Ride_Enemy_Attack`,
#: `W_Ridden_Enemy_Step` and `W_Ride_Enemy_Step` are all 100% num==anim-id, so they look
#: like free coverage. A full sweep with them included says otherwise: they were chosen
#: for **zero** of the 6921 shipped moves -- every id they reach is also reached by
#: `W_Event` or `W_Step`, which outrank them -- while adding roughly six thousand
#: `no-damage-window` denial rows for mount-only animations and doubling the table to
#: 224 KB.
#:
#: They are also semantically wrong for this mod even when they are the only spelling:
#: `W_Ridden_Enemy_Step` is what a mount plays while somebody is riding it, and a
#: possessed creature is not being ridden. Firing one is a softlock the watchdog would
#: have to clean up.
#:
#: Left here as a named constant rather than deleted, because "we tried the ride families
#: and they bought nothing" is the finding, and a future agent reading a five-entry
#: PREFIXES tuple would otherwise re-derive it.
RIDE_PREFIXES_MEASURED_USELESS = (
    'W_Ride_Attack_',
    'W_RideStep',
    'W_Ridden_Enemy_Attack',
    'W_Ride_Enemy_Attack',
    'W_Ridden_Enemy_Step',
    'W_Ride_Enemy_Step',
)

#: TAE ability event type -> index into `TaeAnimEventParams::Args` holding the id it
#: resolves. Decoded from both native dispatchers by `scripts/er-tae-dispatch-decode.py`.
#: Type 5 is the odd one: it is a raw absolute BehaviorParam row id with no
#: `ResolveBehaviorId` call in front of it.
ABILITY_ARG = {1: 2, 2: 2, 5: 1, 123: 1, 304: 1, 307: 2}
RAW_BEHAVIOR_TYPES = {5}
#: TAE event type 304 `ThrowAttackBehavior` marks the throw-result clip, not the grab.
#:
#: This constant used to be called the grab carrier, and reading it that way is what made
#: `allow_grabs` gate nothing. 304 is 573 sites across 90 chrs and 100% of the 4000 band --
#: and swept over the whole corpus, zero of the 108 animations carrying it are fireable
#: under any prefix. That is not a wall the graph puts up; it is that these clips are never
#: addressed by id. `CSChrThrowModule::PlayThrowAnim` reaches them through the two bare
#: names `W_ThrowAtk` (attacker) and `W_ThrowDef` (defender), and which clip that lands on
#: is `ThrowParam.atkAnimId` / `defAnimId` for the row the throw system already picked.
#:
#: The grab itself is somewhere else entirely -- see [`ATK_THROW_FIELD`].
GRAB_EVENT_TYPE = 304

#: The chain window. TAE event type 0 is `ChrActionFlag` and its `params[0]` (`FlagType`)
#: selects a case in `CS::CSChrTaeAnimEvent::_ChrActionFlag` (1.16.2 `0x1404275e0`). The
#: case that means "this attack may now be cancelled into another attack" is **86**, whose
#: body is `actionRequest->taeCancels |= 0x20` -- the bit `CS::CSAiFunc::IsEnableCancelAttack`
#: (`0x140300800` -> `0x1404075a0`, `taeCancels & 0x20 && !(taeCancels >> 0xb & 1)`) reads.
#: That is the game's own per-animation answer to "may this swing be left yet", asked of a
#: creature rather than of the player, which is exactly the question a possessed creature has.
#:
#: **86 is the creature one and 4 is the player one, and they are not interchangeable.**
#: FlagType 4 (`CANCEL_R1_R2_LIGHT_KICK_HEAVY_KICK`) is the combo window everybody means when
#: they say "TAE combo window", and it covers 78.8% of c0000's 5,322 attack animations -- but
#: only 19 of the 6,419 non-player attack animations in this corpus, 0.3%. Building the window
#: on 4 would have produced a table that was empty for almost every creature this mod can
#: possess. 86 covers 91.1% of them. 4 is still accepted as a fallback for the handful that
#: carry it, since both cases mean the same thing to their own kind of character.
#:
#: `GetActiveAnimEvent` (`0x14264e420`) re-executes an event on every frame whose playhead
#: slice `(prevLocalTime, localTime]` overlaps `[startTime, endTime]`, so the window is the
#: event's own start and end in animation-local seconds and nothing has to be derived.
CANCEL_ATTACK_FLAG_TYPES = (86, 4)
#: `params[0]` of a type-0 event, the `FlagType` the dispatcher switches on.
CHR_ACTION_FLAG_ARG = 0

#: Buckets, in the fixed 4-button layout every creature shares.
BUCKET_LIGHT, BUCKET_HEAVY, BUCKET_RANGED, BUCKET_MOVEMENT = 0, 1, 2, 3

#: Reach bands, in absolute metres, because the runtime measures the player-to-target
#: distance in metres too. A per-chr percentile would be wrong here for once: the
#: dispatcher compares reach against a real distance, not against other attacks.
REACH_UNKNOWN, REACH_CLOSE, REACH_MID, REACH_FAR = 0, 1, 2, 3
REACH_CLOSE_MAX, REACH_MID_MAX = 3.0, 8.0

#: Denial reasons. Every one of these is emitted into `er-npc-possess.derived.toml`
#: at runtime with its reason -- nothing is withheld silently.
DENY_NOT_FIREABLE = 1
DENY_NO_CLIP = 2
DENY_NO_DAMAGE_WINDOW = 3
DENY_MISSING_ATK_ROW = 4
DENY_SPEFFECT_ONLY = 5
DENY_UNRESOLVED_BEHAVIOR = 6
#: The animation the throw system plays once a grab has been accepted, reached by the bare
#: names `W_ThrowAtk`/`W_ThrowDef` rather than by id. There is nothing here to fire, which
#: is a different and more useful thing to be told than `not-fireable`. See
#: [`throw_result_clip`]; measured at 108 animations, all in the 4000 band, none fireable.
DENY_THROW_RESULT_CLIP = 10
#: Retired. Reason 7 was `prefix-unreachable`: reachable by the graph but not spellable by
#: the `W_Event%04d` field write. It existed for one commit and is gone because the class is
#: gone -- those ids are now fired, through `PlayAnimationByBehaviorName` with a name built
#: from the prefix this generator resolved. An id that no prefix reaches is `not-fireable`,
#: which it always was. The number is left unused rather than recycled so a table written
#: before the fallback cannot be misread by a parser written after it.
DENY_UNUSABLE_AT_RUNTIME = 9

#: A TAE event that runs to the end of its animation stores `FLT_MAX` as its end time.
#: Taking `max(end)` naively therefore reports every animation in the game as lasting
#: 3.4e38 seconds, which silently collapses the light/heavy score to a constant.
MAX_PLAUSIBLE_EVENT_TIME = 120.0

EVENT_NAME_RE = re.compile(r'^([A-Za-z_]+?)(\d{3,6})$')


def in_bands(anim_id):
    return any(lo <= anim_id <= hi for lo, hi in BANDS)


def band_of(anim_id):
    for name, (lo, hi) in (('attack', ATTACK_BAND), ('grab', GRAB_BAND),
                           ('step', STEP_BAND)):
        if lo <= anim_id <= hi:
            return name
    return None


# ---------------------------------------------------------------------------
# corpus discovery
# ---------------------------------------------------------------------------

def chr_dirs(root, kind):
    """{chrId: [dir, ...]} for `<chr>-<kind>*` under root and its `_chunk_*` shards."""
    out = collections.defaultdict(list)
    for path in glob.glob(f'{root}/*/*-{kind}*') + glob.glob(f'{root}/*-{kind}*'):
        stem = os.path.basename(path).split('-')[0]
        chr_id = stem.split('_')[0]
        if re.fullmatch(r'c\d{4}', chr_id):
            out[chr_id].append(path)
    return out


def tae_paths_for(anibnd_dirs):
    out = []
    for directory in anibnd_dirs:
        out += glob.glob(os.path.join(directory, '**', '*.tae'), recursive=True)
    return sorted(out)


def family_base(variation):
    """The chr id whose `.tae` describes a creature with this `behaviorVariationId`.

    The CHR-ID join is not the animation join, and assuming it was cost this table a third
    of its attacks. A creature like c4351 Godrick Knight owns a model, a skeleton and a
    behaviour graph that declares and can fire all 72 in-band ids -- and its `.anibnd`
    contains `skeleton.hkx` and nothing else. There is no `c4351.tae` because there are no
    c4351 animations: the whole 435x knight family plays out of `c4350.anibnd.dcx`, whose
    `INTERROOT_win64/chr/c4350/tae/c4350.tae` is a megabyte of TimeAct covering all six of
    them.

    `NpcParam.behaviorVariationId` is what names that base -- `<family> * 100 + <variant>`,
    so 43500 is family 435 and its animations ship under `c4350`. Measured over the corpus:
    270 creatures own their TimeAct (their own family base is themselves, e.g. c4160 at
    variation 41600), 133 inherit it, and 5 have none under either id. Before this join
    those 133 reached `classify_chr` with an empty TimeAct dict, so every attack they can
    fire was denied `no-damage-window` and then trimmed out of existence by the in-span
    rule, leaving the twelve `W_Step` walk clips as the entire shipped moveset.

    Returns None for a variation the convention cannot express -- absent, or one of the
    100000+ sentinels `Regulation.resolve` already refuses.
    """
    if variation is None or variation >= 100000 or variation < 0:
        return None
    return f'c{variation // 100 * 10:04d}'


def tae_paths_for_chr(anibnds, chr_id, variation):
    """Every `.tae` describing this creature: its own, else its family base's.

    The single place the fallback is decided, so the generator and the probe cannot drift
    apart and report different movesets for the same creature.
    """
    paths = tae_paths_for(anibnds.get(chr_id, []))
    if paths:
        return paths, chr_id
    base = family_base(variation)
    if base and base != chr_id:
        paths = tae_paths_for(anibnds.get(base, []))
        if paths:
            return paths, base
    return [], None


# ---------------------------------------------------------------------------
# behavior graph: which animation ids are actually FIREABLE
# ---------------------------------------------------------------------------

def fireable_animations(behbnd_dir):
    """{firedId: (playedId, prefixIndex)} for every in-band id this chr can actually fire.

    `firedId` is the number in the event name. `playedId` is the animation the target
    state's `hkbClipGenerator` really plays -- equal to `firedId` for everything except the
    systematic `W_Event3110 -> a000_003000` exception, which is why the table carries both
    rather than assuming identity. `prefixIndex` indexes [`PREFIXES`] and says which
    spelling of the id has a transition behind it on this creature.

    The prefix is resolved from the chr's own event table and never from the band. Bands
    are a FromSoft convention that tells you what to try; only a real `TransitionInfo`
    landing on a real `StateInfo` says what works, and the two disagree constantly --
    c4500 declares `W_Step6000` through `W_Step6023` and can fire none of them, while
    c2120 fires five.

    Also returns the set of in-band ids declared under some prefix with no transition
    behind any spelling, so they are reported as denials instead of vanishing.
    """
    fireable = {}
    declared = set()
    for hkx in sorted(glob.glob(os.path.join(behbnd_dir, 'Behaviors', '*.hkx'))):
        graph = BMAP.Graph(hkx)
        for name in graph.events:
            match = EVENT_NAME_RE.match(name)
            if match and match.group(1) in PREFIXES and in_bands(int(match.group(2))):
                declared.add(int(match.group(2)))
        for event, targets in graph.event_to_anim().items():
            match = EVENT_NAME_RE.match(event)
            if not match or match.group(1) not in PREFIXES:
                continue
            fired = int(match.group(2))
            if not in_bands(fired):
                continue
            rank = PREFIXES.index(match.group(1))
            anims = sorted({a for t in targets for a in t['anims']})
            best = fireable.get(fired)
            # Lower index wins, so `W_Event` beats everything and the ride families lose
            # to everything. A tie inside one prefix keeps the first target seen.
            if best is None or rank < best[1]:
                fireable[fired] = (anims, rank)

    resolved = {}
    for fired, (anims, rank) in fireable.items():
        if not anims:
            resolved[fired] = (None, rank)      # a real state that plays no clip
        elif fired in anims:
            resolved[fired] = (fired, rank)
        else:
            resolved[fired] = (min(anims), rank)  # the 3110 case
    return resolved, declared


# ---------------------------------------------------------------------------
# TimeAct: which animations open a damage window, and for how long
# ---------------------------------------------------------------------------

def tae_facts(tae_path):
    """{animId: {'duration': f, 'abilities': [...], 'grab': bool, 'chain_from': f|None}}."""
    _, anims = TAE.parse(tae_path)
    out = {}
    for raw_id, events in anims.items():
        anim_id = raw_id % 1000000
        if not in_bands(anim_id):
            continue
        duration = 0.0
        abilities = []
        grab = False
        chain_from = None
        for event in events:
            for stamp in (event.start, event.end):
                value = float(stamp)
                if 0.0 <= value <= MAX_PLAUSIBLE_EVENT_TIME:
                    duration = max(duration, value)
            # The earliest moment the game itself says this attack may be left. Earliest
            # rather than latest because a multi-window animation opens the door more than
            # once and the first opening is the one a player pressing a button will reach;
            # the runtime already stops treating the move as committed once the animation
            # has ended, so the closing edge buys nothing worth a second column.
            if event.type == TAE.JUMPTABLE_EVENT_TYPE and len(event.params) >= 4:
                flag = struct.unpack_from('<i', event.params, CHR_ACTION_FLAG_ARG * 4)[0]
                start = float(event.start)
                if flag in CANCEL_ATTACK_FLAG_TYPES and 0.0 <= start <= MAX_PLAUSIBLE_EVENT_TIME:
                    chain_from = start if chain_from is None else min(chain_from, start)
            arg = ABILITY_ARG.get(event.type)
            if arg is None:
                continue
            if len(event.params) < (arg + 1) * 4:
                continue
            value = struct.unpack_from('<i', event.params, arg * 4)[0]
            abilities.append((event.type, value))
            if event.type == GRAB_EVENT_TYPE:
                grab = True
        record = out.setdefault(anim_id, {'duration': 0.0, 'abilities': [],
                                          'grab': False, 'chain_from': None})
        record['duration'] = max(record['duration'], duration)
        record['abilities'] += abilities
        record['grab'] = record['grab'] or grab
        record['chain_from'] = _earliest(record['chain_from'], chain_from)
    return out


def _earliest(left, right):
    """min() that treats None as "not measured" rather than as zero."""
    if left is None:
        return right
    if right is None:
        return left
    return min(left, right)


# ---------------------------------------------------------------------------
# regulation.bin: behaviour rows, damage numbers and hit-capsule radii
# ---------------------------------------------------------------------------

ATK_DAMAGE_FIELDS = ('atkPhys', 'atkMag', 'atkFire', 'atkThun')
ATK_RADIUS_FIELDS = ('hit0_Radius', 'hit1_Radius', 'hit2_Radius', 'hit3_Radius')
#: The column that makes an attack a grab. Non-zero means the hit, when it lands, is
#: handed to the throw system instead of to the damage calculation -- see [`Regulation`].
ATK_THROW_FIELD = 'throwTypeId'


class Regulation:
    """The three param tables this join needs, read once and shared across chrs."""

    def __init__(self, path=None):
        files = PR.load(path)
        npc, _, _ = PR.rows(PR.param_bytes(files, 'NpcParam'),
                            ['behaviorVariationId'])
        self.npc_variation = {}
        for row in npc:
            self.npc_variation.setdefault(row['id'] // 10000, []).append(
                row['behaviorVariationId'])
        behavior, _, _ = PR.rows(PR.param_bytes(files, 'BehaviorParam'),
                                 ['variationId', 'behaviorJudgeId', 'refType', 'refId'])
        self.behavior = {row['id']: (row['refType'], row['refId']) for row in behavior}
        atk, _, _ = PR.rows(
            PR.param_bytes(files, 'AtkParam_Npc'),
            list(ATK_DAMAGE_FIELDS) + list(ATK_RADIUS_FIELDS) + [ATK_THROW_FIELD])
        self.atk = {
            row['id']: (
                sum(row[f] for f in ATK_DAMAGE_FIELDS),
                max(row[f] for f in ATK_RADIUS_FIELDS),
                int(row[ATK_THROW_FIELD]),
            )
            for row in atk
        }
        # The grab join. `ApplyDamage` reads `AtkParam.throwTypeId` off the hit that landed
        # and hands it to `CSChrThrowModule::InitThrow`; `ValidateAttemptAndReturnParamId`
        # then scans ThrowParam for a row matching (attacker npcId, victim npcId,
        # throwTypeId). No row, no throw -- so an attack whose `throwTypeId` names nothing
        # here is not a grab however the flag reads.
        throw, _, _ = PR.rows(PR.param_bytes(files, 'ThrowParam'),
                              ['AtkChrId', 'DefChrId', 'throwTypeId', 'Dist'])
        self.throw = collections.defaultdict(list)
        for row in throw:
            if row['throwTypeId'] == 0 and row['AtkChrId'] == 0:
                continue                                 # the null/default row
            self.throw[(row['AtkChrId'], row['throwTypeId'])].append(
                (row['DefChrId'], float(row['Dist'])))
        # A projectile's reach is its own travel distance, not a hit capsule. Calling
        # every bullet "far" would send a 2-metre spit and a cross-arena breath to the
        # same distance band, and the short one whiffs every time the dispatcher picks
        # it at range.
        bullet, _, _ = PR.rows(PR.param_bytes(files, 'Bullet'),
                               ['dist', 'atkId_Bullet'])
        self.bullet_reach = {row['id']: float(row['dist']) for row in bullet}
        self.bullet_damage = {
            row['id']: self.atk.get(row['atkId_Bullet'], (0, 0.0))[0] for row in bullet
        }

    def throws_for(self, chr_num, throw_type_id):
        """[(victimChrId, rangeDecimetres)] a throw of this type by this chr can complete.

        Empty when `throw_type_id` is 0 (an ordinary hit) or when no `ThrowParam` row pairs
        this attacker with this throw type -- measured, 10 of the 179 non-zero
        `throwTypeId`s on fireable attacks name nothing. Those are not grabs: the native
        scan in `CSThrowNode::ValidateAttemptAndReturnParamId` finds no row, returns -1,
        and `StartThrow` gives up. Calling them grabs would put a label on a swing that can
        never become one.

        Decimetres so the table stays integers-only, per this file's header.
        """
        if not throw_type_id:
            return []
        return [(victim, int(round(distance * 10)))
                for victim, distance in self.throw.get((chr_num, throw_type_id), ())]

    def variation_for(self, chr_id):
        """`NpcParam.behaviorVariationId` for a chr, by majority of its NpcParam rows."""
        values = self.npc_variation.get(int(chr_id[1:]), [])
        if not values:
            return None
        return collections.Counter(values).most_common(1)[0][0]

    def resolve(self, event_type, value, variation):
        """(refType, refId) for one TAE ability event, or None when it resolves to nothing.

        Creature `ResolveBehaviorId` is `(variationId + 200000) * 1000 + judgeId`;
        type 5 skips it entirely and carries an absolute row id.
        """
        if event_type in RAW_BEHAVIOR_TYPES:
            return self.behavior.get(value)
        if variation is None or variation >= 100000:
            return None
        return self.behavior.get((variation + 200000) * 1000 + value)


# ---------------------------------------------------------------------------
# classification
# ---------------------------------------------------------------------------

def reach_band(metres, is_bullet):
    if is_bullet:
        return REACH_FAR
    if metres is None or metres <= 0.0:
        return REACH_UNKNOWN
    if metres < REACH_CLOSE_MAX:
        return REACH_CLOSE
    if metres < REACH_MID_MAX:
        return REACH_MID
    return REACH_FAR


def throw_result_clip(tae, anim_id):
    """Is this animation the one the throw system plays, rather than one you can fire?

    TAE event 304 `ThrowAttackBehavior` marks it. Measured over the whole corpus: 108
    animations carry 304, all of them in the 4000 band, and not one of them is fireable
    under any prefix -- because they are never addressed by id at all.
    `CSChrThrowModule::PlayThrowAnim` reaches them by the two bare, un-numbered names
    `W_ThrowAtk` and `W_ThrowDef`, and which clip that resolves to is decided by the
    `ThrowParam` row the throw system already chose. So the right denial for one of these
    is not "the graph cannot reach it" -- it is "there is nothing here to fire".
    """
    return bool((tae.get(anim_id) or {}).get('grab'))


def classify_chr(fireable, declared, tae, regulation, variation, chr_num):
    """-> (entries, denials) for one creature.

    entries: [(fired, played, bucket, rank, reach, throws, prefix)]
    denials: [(fired, reason)]
    """
    # The throw-result clips are listed whether or not the graph declares a name for them.
    #
    # `declared` comes from the graph's event-name table, and most 4000-band clips have no
    # event name at all -- so left to that set they would simply vanish, and the player who
    # knows Malenia's grab is a4100 would find nothing in the derived file saying why it is
    # not on offer. They are the single most-asked-about hole in a boss's moveset, so they
    # get an entry with a reason that is true of them rather than silence.
    reasons = {a: DENY_NOT_FIREABLE for a in declared - set(fireable)}
    for anim_id in tae:
        if throw_result_clip(tae, anim_id) and anim_id not in fireable:
            reasons[anim_id] = DENY_THROW_RESULT_CLIP
    denials = sorted(reasons.items())
    candidates = []
    for fired in sorted(fireable):
        played, prefix = fireable[fired]
        if played is None:
            denials.append((fired, DENY_NO_CLIP))
            continue
        facts = tae.get(played) or tae.get(fired) or {}
        # The chain window belongs to the clip that plays, not to the id that is fired --
        # `W_Event3110` plays `a000_003000`, and the TimeAct the runtime will be reading is
        # 3000's. `facts` already resolves in that order for the same reason.
        chain_from = facts.get('chain_from')
        # DEDUPE. A multi-hit swing repeats the same `(type, behaviorId)` pair once per
        # hitbox activation -- c4500's a3035 carries the same two bullet ids 44 times.
        # Summing the duplicates would rank an attack by how many times the TAE
        # re-arms its hitbox rather than by how hard it hits.
        abilities = sorted(set(facts.get('abilities', [])))
        duration = facts.get('duration', 0.0)
        throws = []
        damage, bullet_damage, reach = 0, 0, 0.0
        melee_any = melee_damaging = bullet = False
        missing_atk = unresolved = False
        for event_type, value in abilities:
            resolved = regulation.resolve(event_type, value, variation)
            if resolved is None:
                unresolved = True
                continue
            ref_type, ref_id = resolved
            if ref_type == 0:
                row = regulation.atk.get(ref_id)
                if row is None:
                    missing_atk = True
                    continue
                melee_any = True
                # The grab. A non-zero `throwTypeId` means this hit does not go to the
                # damage calculation at all: `ApplyDamage` hands it to
                # `CSChrThrowModule::InitThrow` first and returns early when the throw is
                # accepted. Which is why most of these rows carry zero damage -- the damage
                # arrives later, from the throw-result clip's own hitbox.
                throws += regulation.throws_for(chr_num, row[2])
                if row[0] > 0:
                    # A zero-damage row is a marker, not a hit. c4500's judge 900 resolves
                    # to AtkParam_Npc 4500900: damage 0, capsule radius 9.0, and it is
                    # attached to nearly every one of the dragon's attacks. Counting its
                    # radius would report the whole moveset as long-reach.
                    melee_damaging = True
                    damage += row[0]
                    reach = max(reach, row[1])
            elif ref_type == 1:
                bullet = True
                bullet_damage += regulation.bullet_damage.get(ref_id, 0)
                reach = max(reach, regulation.bullet_reach.get(ref_id, 0.0))
        throws = sorted(set(throws))
        if band_of(fired) == 'step':
            candidates.append((fired, played, BUCKET_MOVEMENT, 0.0, REACH_UNKNOWN,
                               (), prefix, chain_from))
            continue
        if not (melee_any or bullet):
            if missing_atk:
                denials.append((fired, DENY_MISSING_ATK_ROW))
            elif unresolved:
                denials.append((fired, DENY_UNRESOLVED_BEHAVIOR))
            elif abilities:
                denials.append((fired, DENY_SPEFFECT_ONLY))
            else:
                denials.append((fired, DENY_NO_DAMAGE_WINDOW))
            continue
        # A ranged option is one that reaches through a bullet and not through a
        # damaging melee capsule; a breath attack that does both stays melee, because
        # its damage is delivered where the creature is standing.
        is_ranged = bullet and not melee_damaging
        bucket = BUCKET_RANGED if is_ranged else None
        score = duration * float(damage + bullet_damage)
        candidates.append((fired, played, bucket, score,
                           reach_band(reach, is_ranged), tuple(throws), prefix,
                           chain_from))

    # Light vs heavy by per-CHR percentile of duration x damage, so a rat and a
    # dragon are comparable: absolute damage would put every one of the rat's
    # attacks in "light" and every one of the dragon's in "heavy".
    #
    # Split by index, not by value. A value cutoff (`score >= median`) collapses
    # whenever scores tie -- and they tie constantly, because a chr's attacks share
    # AtkParam rows -- which sends the whole tied block into one bucket and leaves the
    # other nearly empty.
    melee = sorted((c for c in candidates if c[2] is None), key=lambda c: (c[3], c[0]))
    heavy_from = (len(melee) + 1) // 2
    heavy = {c[0] for c in melee[heavy_from:]}
    resolved = []
    for fired, played, bucket, score, reach, throws, prefix, chain in candidates:
        if bucket is None:
            bucket = BUCKET_HEAVY if fired in heavy else BUCKET_LIGHT
        resolved.append((fired, played, bucket, score, reach, throws, prefix, chain))

    # Rank inside a bucket: ascending score, then ascending animation id. Fully
    # deterministic, so the same corpus always produces the same table and the
    # regeneration test is meaningful.
    entries = []
    for bucket in (BUCKET_LIGHT, BUCKET_HEAVY, BUCKET_RANGED, BUCKET_MOVEMENT):
        members = sorted((c for c in resolved if c[2] == bucket),
                         key=lambda c: (c[3], c[0]))
        for rank, (fired, played, _, _, reach, throws, prefix, chain) in enumerate(members):
            entries.append((fired, played, bucket, rank, reach, throws, prefix, chain))
    entries.sort()

    # TRIM DENIALS to the span the creature actually uses.
    #
    # A denial earns its place by being a hole a player could notice: an id sitting among the
    # moves that work, which they might reasonably expect and which is missing. Outside that
    # span it is a fact about the shared behaviour-graph string table rather than about this
    # creature, and there are a lot of those -- every chr's behbnd carries roughly the same
    # declared vocabulary whether or not this one has states for it.
    #
    # Measured, both times this trim was widened. `not-fireable` alone was 62 KB, half the
    # table, of 270 near-identical copies of one list. Widening the prefix set then produced
    # 9,298 `no-damage-window` rows, of which 791 are in-span and 8,507 are not -- ids past
    # everything the creature can do, reachable only because `W_Attack` spans 3000-4602 and
    # carrying no damage window because they are not that creature's animations.
    #
    # The in-span 791 are kept in full, which is the set that existed before the prefix work,
    # so nothing a player could act on has been dropped to save bytes.
    if entries:
        low, high = entries[0][0], entries[-1][0]
        denials = [(a, r) for a, r in denials if low <= a <= high]
    denials.sort()
    return entries, denials


# ---------------------------------------------------------------------------
# emit
# ---------------------------------------------------------------------------

def format_table(per_chr):
    lines = [
        '# er-npc-possess moveset table -- GENERATED, do not hand-edit.',
        '# Regenerate: scripts/er-moveset-table-gen.py --out '
        'crates/er-npc-possess/data/moveset.tbl',
        '# Integers only. No game bytes: every number here is an animation id (a BND',
        '# entry filename) or a decision this generator made about one.',
        '#',
        '# One line per creature:  <chrid> <entry>...',
        '#   entry  = <fired>[=<played>][<window>][<grab>]:<bucket>:<rank>:<reach>[:<prefix>]',
        '#   window = w<centiseconds> -- the earliest moment the GAME says this attack may',
        '#   be cancelled into another one: the start of its TAE type-0 ChrActionFlag event',
        '#   with FlagType 86, the case whose handler sets the taeCancels bit 0x20 that',
        '#   CSAiFunc::IsEnableCancelAttack reads. FlagType 4 (the PLAYER combo window) is',
        '#   accepted as a fallback; it covers 78.8% of c0000 and 0.3% of everything else.',
        '#   Absent means unmeasured, not zero: the runtime keeps such a move committed for',
        '#   its whole length rather than guessing a window for it.',
        '#   grab   = g<victimChrId>,<rangeDecimetres>[+<victimChrId>,<rangeDecimetres>]...',
        '#   denial = !<fired>:<reason>',
        '#   "-"    = considered, nothing fireable -- a variant with no animations of its own',
        '#   "=played" appears only when the graph plays a DIFFERENT animation than the',
        '#   one named in the event -- the W_Event3110 -> a000_003000 exception.',
        '#   "g..."  marks a THROW INITIATOR: an ordinary attack whose AtkParam_Npc row has',
        '#   throwTypeId != 0 AND a ThrowParam row pairing this chr with a victim. Landing it',
        '#   is what starts a grab; the 4000-band clip is what the throw system plays after.',
        '#   The victim chr id is ThrowParam.DefChrId (0 = the player) and the range is its',
        '#   Dist. Gated at runtime by allow_grabs.',
        f'#   bucket {BUCKET_LIGHT}=light {BUCKET_HEAVY}=heavy '
        f'{BUCKET_RANGED}=ranged {BUCKET_MOVEMENT}=movement',
        f'#   reach  {REACH_UNKNOWN}=unknown {REACH_CLOSE}=close '
        f'{REACH_MID}=mid {REACH_FAR}=far',
        f'#   reason {DENY_NOT_FIREABLE}=not-fireable {DENY_NO_CLIP}=no-clip '
        f'{DENY_NO_DAMAGE_WINDOW}=no-damage-window '
        f'{DENY_MISSING_ATK_ROW}=missing-atk-row',
        f'#          {DENY_SPEFFECT_ONLY}=speffect-only '
        f'{DENY_UNRESOLVED_BEHAVIOR}=unresolved-behavior '
        f'{DENY_UNUSABLE_AT_RUNTIME}=unusable-at-runtime (watchdog, runtime only)',
        f'#          {DENY_THROW_RESULT_CLIP}=throw-result-clip (played BY the throw '
        f'system, never fired)',
        '#',
        '# prefix 0=W_Event (fired by writing CSChrEventModule+0x18, no game address) and',
        '# 1..10 = ' + ' '.join(f'{i}={p}' for i, p in enumerate(PREFIXES) if i),
        '# A non-zero prefix is fired through PlayAnimationByBehaviorName with a name built',
        '# from the id. Resolved per creature from its OWN event table, never from the band.',
        '# Denials are listed only WITHIN the span of ids a creature really uses. Outside it they',
        '# are a property of the shared graph string table rather than of this creature: 8,507 of',
        '# 9,298 no-damage-window rows sat past everything the creature can do.',
        f'v{TABLE_VERSION}',
    ]
    for chr_id in sorted(per_chr):
        entries, denials = per_chr[chr_id]
        if not entries and not denials:
            continue
        if not entries:
            # Nothing usable. Almost always a D-class variant: it owns a model and a
            # skeleton but no animations of its own, and its behbnd carries the shared
            # generic graph, so the same ~54 attack ids come back declared-but-stateless
            # for every one of them. Enumerating that identical list 130 times would be
            # 7000 rows of noise saying one thing, so it is said once, per creature: this
            # one was looked at and has no fireable moveset.
            lines.append(f'{int(chr_id[1:])} -')
            continue
        parts = []
        for fired, played, bucket, rank, reach, throws, prefix, chain in entries:
            head = str(fired) if played == fired else f'{fired}={played}'
            # CENTISECONDS, and absent when unmeasured. `w0` would be a claim -- "chainable
            # from the first frame" -- and the runtime treats a missing window as committed
            # for the whole clip instead, which is the honest reading of "we do not know".
            if chain is not None:
                head += f'w{int(round(chain * 100))}'
            # The prefix column is omitted for `W_Event`, which is both the common case
            # and the one that costs no game address -- so a four-field entry reads as
            # "fired by the field write" and a five-field one as "fired by name".
            tail = '' if prefix == 0 else f':{prefix}'
            grab = ''
            if throws:
                grab = 'g' + '+'.join(f'{victim},{reach_dm}'
                                      for victim, reach_dm in throws)
            parts.append(f'{head}{grab}:{bucket}:{rank}:{reach}{tail}')
        parts += [f'!{fired}:{reason}' for fired, reason in denials]
        lines.append(f'{int(chr_id[1:])} ' + ' '.join(parts))
    return '\n'.join(lines) + '\n'


def _one(args):
    chr_id, behbnd, taes, variation = args
    try:
        fireable, declared = fireable_animations(behbnd)
    except Exception as error:                       # a chr whose graph will not parse
        return chr_id, None, repr(error)[:120]
    tae = {}
    for path in taes:
        try:
            for anim, facts in tae_facts(path).items():
                merged = tae.setdefault(anim, {'duration': 0.0, 'abilities': [],
                                               'grab': False, 'chain_from': None})
                merged['duration'] = max(merged['duration'], facts['duration'])
                merged['abilities'] += facts['abilities']
                merged['grab'] = merged['grab'] or facts['grab']
                merged['chain_from'] = _earliest(merged['chain_from'],
                                                 facts['chain_from'])
        except Exception:
            continue
    return chr_id, (fireable, declared, tae, variation), None


def generate(root, only=None, jobs=10, regulation_path=None):
    regulation = Regulation(regulation_path)
    behbnds = chr_dirs(root, 'behbnd')
    anibnds = chr_dirs(root, 'anibnd')
    work = []
    for chr_id in sorted(behbnds):
        if only and chr_id not in only:
            continue
        if chr_id == 'c0000':                        # the player is out of scope
            continue
        variation = regulation.variation_for(chr_id)
        # The TimeAct join goes through `behaviorVariationId`, not through the chr id --
        # see `family_base`. The fallback is only taken when the chr-id join found nothing
        # at all, so a creature shipping its own TimeAct is never second-guessed.
        taes, _owner = tae_paths_for_chr(anibnds, chr_id, variation)
        work.append((chr_id, sorted(behbnds[chr_id])[0], taes, variation))
    per_chr, errors = {}, {}
    # Serial when asked for one job, and the check gate asks for one. `multiprocessing`
    # cannot pickle `_one` when this file is loaded as a module by path rather than
    # imported by name, which is exactly how `scripts/check-moveset-table.py` uses it --
    # so the fast subset regeneration would die in the pool rather than in the work.
    results = map(_one, work) if jobs <= 1 else None
    with contextlib.ExitStack() as stack:
        if results is None:
            # Fork, not the 3.14 default forkserver. A forkserver worker is a fresh
            # interpreter that re-imports the module a task came from by name, and this
            # file's name has hyphens in it, so it is not importable by name at all --
            # every worker dies with ModuleNotFoundError the moment a task arrives.
            # `scripts/check-moveset-table.py` loads this module by path, which is when
            # that bites. Fork inherits the parent's already-loaded module instead.
            pool = stack.enter_context(
                multiprocessing.get_context('fork').Pool(jobs)
            )
            results = pool.imap_unordered(_one, work, chunksize=1)
        for chr_id, payload, error in results:
            if error:
                errors[chr_id] = error
                continue
            fireable, declared, tae, variation = payload
            per_chr[chr_id] = classify_chr(fireable, declared, tae, regulation,
                                           variation, int(chr_id[1:]))
            print(f'{chr_id} {len(per_chr[chr_id][0])}+{len(per_chr[chr_id][1])}d',
                  file=sys.stderr, flush=True)
    return per_chr, errors


def event_histogram(root):
    """TAE event types seen inside the covered bands, so denial classes are grounded
    in what the corpus actually contains rather than in a guess about what it might."""
    anibnds = chr_dirs(root, 'anibnd')
    counts, chrs = collections.Counter(), collections.defaultdict(set)
    for chr_id, dirs in sorted(anibnds.items()):
        if chr_id == 'c0000':
            continue
        for path in tae_paths_for(dirs):
            try:
                _, anims = TAE.parse(path)
            except Exception:
                continue
            for raw_id, events in anims.items():
                if not in_bands(raw_id % 1000000):
                    continue
                for event in events:
                    counts[event.type] += 1
                    chrs[event.type].add(chr_id)
    for event_type, count in counts.most_common():
        print(f'{event_type}\t{count}\t{len(chrs[event_type])}')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', default=CORPUS_ROOT)
    parser.add_argument('--out', default='-')
    parser.add_argument('--only')
    parser.add_argument('--jobs', type=int,
                        default=int(os.environ.get('SWEEP_JOBS', '10')))
    parser.add_argument('--regulation')
    parser.add_argument('--event-histogram', action='store_true')
    options = parser.parse_args()
    if options.event_histogram:
        event_histogram(options.root)
        raise SystemExit(0)
    only = set(options.only.split(',')) if options.only else None
    table, failures = generate(options.root, only, options.jobs, options.regulation)
    text = format_table(table)
    if options.out == '-':
        sys.stdout.write(text)
    else:
        with open(options.out, 'w', encoding='utf-8') as handle:
            handle.write(text)
    total = sum(len(v[0]) for v in table.values())
    denied = sum(len(v[1]) for v in table.values())
    print(f'# {len(table)} chrs, {total} entries, {denied} denials, '
          f'{len(text)} bytes', file=sys.stderr)
    for chr_id, error in sorted(failures.items()):
        print(f'# FAILED {chr_id}: {error}', file=sys.stderr)
