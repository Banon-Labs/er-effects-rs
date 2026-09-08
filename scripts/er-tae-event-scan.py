#!/usr/bin/env python3
"""Offline scan of Elden Ring `.tae` (TimeAct) files for TAE events / JumpTable ids.

Corrected 2026-09-01. The i-frame carrier is not "TAE event type 8". It is
**TAE event type 0 (JumpTable) with first param == 8**:

  * The master TAE event-type dispatcher is inside `ExecuteThreadOne`
    (1.16.2 `0x14042e1c2`): `cmp $0xee` -> byte index table `0x14042eda0`
    -> dword jump table `0x14042ece8`. **Event type 0 alone** lands on
    `0x14042e1f6 -> jmp 0x1404275e0` (`0ChrActionFlag`); event type 300 lands on
    `ActivateChrActionFlagEarly` `0x140425ba0`.
  * `0ChrActionFlag` then switches on the event's **params[0]** (`mov 0x8(%r13),%r15`
    = `TaeAnimEventParams::Args`, `mov (%r15),%eax`), `dec`, `cmp $0x8e`, jump table
    `0x140428650`. Index 7 (params[0]==8) is `0x140427860`:
    `orq $0x2,0x40(%rbx)` -> `CSChrActionFlagModule::actionModifiersFlags |= 2`.
  * `CS::ChrIns::IsImmuneToAttack` `0x1403f3b90` returns true when that bit is set
    (and the incoming ATK_PARAM_ST has `isDisableNoDamage == 0`).

So `--jumptable 8` is the i-frame query. `--type 8` is a different, unused event.

Layout transcribed from WitchyBND's SoulsFormats, SDT/ER branch only
(SoulsFormats/Formats/TAE/{TAE,Animation,Event}.cs). Read-only: it parses offsets,
counts, event type ints and param bytes, and never writes to the corpus.

    python3 scripts/er-tae-event-scan.py --selftest <file.tae>
    python3 scripts/er-tae-event-scan.py --jumptable 8 --root <unpacked-chr-tree> --all
    python3 scripts/er-tae-event-scan.py --jumptable-histogram --root <tree> --all
"""
import argparse
import collections
import glob
import os
import struct
import sys

#: TAE event type 0 == JumpTable; its params[0] selects the ChrActionFlag case.
JUMPTABLE_EVENT_TYPE = 0
#: JumpTable id whose dispatcher case ORs 0x2 into actionModifiersFlags (i-frames).
IFRAME_JUMPTABLE_ID = 8
#: ER/Sekiro TAE version. DS3 is 0x1000C; anything else here is a different game.
ER_TAE_VERSION = 0x1000D
#: Header field offsets (SDT/ER, 64-bit, little-endian).
OFF_VERSION, OFF_TAE_ID, OFF_ANIM_COUNT, OFF_ANIMS_OFFSET = 0x08, 0x50, 0x54, 0x58
#: Per-animation entry in the anim table: varint id + varint offset.
ANIM_ENTRY_SIZE = 16
#: Within an animation header: 4 varints then eventCount.
OFF_EVENT_COUNT_IN_ANIM = 0x20
#: Per-event header: startTimeOffset, endTimeOffset, eventDataOffset.
EVENT_HEADER_SIZE, OFF_EVENT_DATA = 24, 16
#: SoulsFormats' own assertion: an event's params always follow its 16-byte data header.
EVENT_PARAMS_DELTA = 16
#: How many param bytes to retain per event (enough for every ChrActionFlag case).
PARAM_BYTES = 32
#: Sanity ceilings -- a real chr TAE is nowhere near these.
MAX_ANIMS, MAX_EVENTS, MAX_EVENT_TYPE = 100000, 100000, 100000


TaeEvent = collections.namedtuple('TaeEvent', 'type start end params')


def jumptable_id(event):
    """The ChrActionFlag JumpTable id of a type-0 event, else None."""
    if event.type != JUMPTABLE_EVENT_TYPE or len(event.params) < 4:
        return None
    return _i32(event.params, 0)


class BadTae(Exception):
    """The file is not an ER TAE, or its offsets do not survive bounds checks."""


def _i32(b, o):
    return struct.unpack_from('<i', b, o)[0]


def _i64(b, o):
    return struct.unpack_from('<q', b, o)[0]


def parse(path):
    """Return (taeId, {animationId: [eventType, ...]}). Raises BadTae on anything unexpected."""
    with open(path, 'rb') as handle:
        b = handle.read()
    if b[:4] != b'TAE ':
        raise BadTae('not a TAE (magic %r)' % b[:4])
    if b[4] != 0:
        raise BadTae('big-endian TAE unsupported')
    if b[7] != 0xFF:
        raise BadTae('32-bit TAE unsupported')
    version = struct.unpack_from('<I', b, OFF_VERSION)[0]
    if version != ER_TAE_VERSION:
        raise BadTae('version 0x%x is not ER/SDT 0x%x' % (version, ER_TAE_VERSION))

    tae_id = _i32(b, OFF_TAE_ID)
    anim_count = _i32(b, OFF_ANIM_COUNT)
    anims_offset = _i64(b, OFF_ANIMS_OFFSET)
    if not 0 <= anim_count < MAX_ANIMS:
        raise BadTae('implausible animation count %d' % anim_count)
    if not 0 <= anims_offset < len(b):
        raise BadTae('anim table offset %d outside %d-byte file' % (anims_offset, len(b)))

    animations = {}
    for index in range(anim_count):
        entry = anims_offset + index * ANIM_ENTRY_SIZE
        if entry + ANIM_ENTRY_SIZE > len(b):
            raise BadTae('anim entry %d outside file' % index)
        anim_id = _i64(b, entry)
        anim_offset = _i64(b, entry + 8)
        if not 0 <= anim_offset < len(b) - OFF_EVENT_COUNT_IN_ANIM - 4:
            raise BadTae('anim %d header offset outside file' % anim_id)
        event_headers_offset = _i64(b, anim_offset)
        event_count = _i32(b, anim_offset + OFF_EVENT_COUNT_IN_ANIM)
        if not 0 <= event_count < MAX_EVENTS:
            raise BadTae('anim %d implausible event count %d' % (anim_id, event_count))
        types = []
        for event_index in range(event_count):
            header = event_headers_offset + event_index * EVENT_HEADER_SIZE
            if header + EVENT_HEADER_SIZE > len(b):
                raise BadTae('anim %d event header outside file' % anim_id)
            data_offset = _i64(b, header + OFF_EVENT_DATA)
            if not 0 <= data_offset < len(b) - 4:
                raise BadTae('anim %d event data offset outside file' % anim_id)
            event_type = _i32(b, data_offset)
            if not 0 <= event_type <= MAX_EVENT_TYPE:
                raise BadTae('anim %d implausible event type %d' % (anim_id, event_type))
            params_offset = _i64(b, data_offset + 8)
            if params_offset != data_offset + EVENT_PARAMS_DELTA:
                raise BadTae('anim %d params offset %d != data %d + %d'
                             % (anim_id, params_offset, data_offset, EVENT_PARAMS_DELTA))
            start = struct.unpack_from('<f', b, _i64(b, header))[0]
            end = struct.unpack_from('<f', b, _i64(b, header + 8))[0]
            types.append(TaeEvent(event_type, start, end,
                                  b[params_offset:params_offset + PARAM_BYTES]))
        animations[anim_id] = types
    return tae_id, animations


#: TAE event types that can produce an offensive/ability effect, decoded from the
#: two native dispatchers by `scripts/er-tae-dispatch-decode.py`.  `arg` is the index
#: into `TaeAnimEventParams::Args` (an int array) holding the id the event resolves,
#: or None when the id does not come from the event's own args.
ABILITY_EVENTS = {
    1:   ('AttackBehavior',        0x1404266d0, 2,    'ResolveBehaviorId -> BehaviorParam'),
    2:   ('BulletBehavior',        0x140426e60, 2,    'ResolveBehaviorId -> BehaviorParam -> BulletParam'),
    5:   ('CommonBehavior',        0x1404269e0, 1,    'RAW -> BehaviorParam (no ResolveBehaviorId)'),
    64:  ('CastHighlightedMagic',  0x140429db0, None, 'CSChrMagicModule::GetActiveSpell -> MagicParam'),
    65:  ('ConsumeCurrentGoods',   0x14042c4f0, None, 'ChrIns::GetToUseItemId -> EquipParamGoods'),
    66:  ('AddSpEffect',           0x14042bfd0, 0,    'ApplySpEffect -> SpEffectParam'),
    67:  ('AddSpEffect2',          0x14042bfd0, 0,    'ApplySpEffect -> SpEffectParam'),
    123: ('SpawnFFXBySpEffect2',   0x140426ca0, 1,    'repack -> BulletBehavior (SpEffect-gated)'),
    302: ('ApplySpEffect302',      0x1403e8be0, 0,    'ApplySpEffect -> SpEffectParam'),
    304: ('ThrowAttackBehavior',   0x14042c0f0, 1,    'ResolveBehaviorId -> BehaviorParam -> throw hitbox'),
    307: ('AttackBehaviorFlagged', 0x14042a580, 2,    'ResolveBehaviorId(flags) or vf FUN_1403ef450'),
    330: ('ConsumeFp',             0x14047f640, 0,    'CSChrSuperArmorModule FP drain'),
    331: ('ApplySpEffect331',      0x1403e8be0, 0,    'ApplySpEffect -> SpEffectParam'),
    340: ('SpawnNpcItemLot',       0x140429a60, 0,    'NpcParam itemLotId -> ItemLotParam'),
    401: ('ApplySpEffect401',      0x1403e8be0, 0,    'ApplySpEffect -> SpEffectParam'),
    785: ('SpawnChrFinderBullet',  0x14042a3b0, 0,    'MultiTargetShootBullet -> BulletParam'),
    903: ('HavokThrowUnk903',      0x140429100, 0,    'Havok throw / grab networking'),
}


def _chr_id_of(path):
    """`.../c3200.tae` -> `c3200`."""
    return os.path.splitext(os.path.basename(path))[0]


def ability_report(paths, skip_player=True):
    """Per-ability-event-type site counts across a set of `.tae` files."""
    sites = collections.Counter()
    chrs = collections.defaultdict(set)
    arg_values = collections.defaultdict(collections.Counter)
    failures = []
    scanned = 0
    for path in paths:
        chr_id = _chr_id_of(path)
        if skip_player and chr_id.startswith('c0000'):
            continue
        try:
            _, animations = parse(path)
        except (BadTae, struct.error) as error:
            failures.append((path, error))
            continue
        scanned += 1
        for events in animations.values():
            for event in events:
                if event.type not in ABILITY_EVENTS:
                    continue
                sites[event.type] += 1
                chrs[event.type].add(chr_id)
                index = ABILITY_EVENTS[event.type][2]
                if index is not None and len(event.params) >= 4 * index + 4:
                    arg_values[event.type][_i32(event.params, 4 * index)] += 1
    return scanned, sites, chrs, arg_values, failures


def _tae_paths(root, chr_ids):
    """Every <chr>.tae under an unpacked chr tree, for the requested chr ids."""
    paths = []
    for chr_id in chr_ids:
        matched = glob.glob(os.path.join(root, '**', '%s.tae' % chr_id), recursive=True)
        paths.extend(sorted(matched))
    return paths


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('paths', nargs='*', help='.tae files to scan')
    parser.add_argument('--type', type=int, default=None, help='TAE event type to count')
    parser.add_argument('--jumptable', type=int, default=None,
                        help='count type-0 JumpTable events with this id (%d == i-frames)'
                             % IFRAME_JUMPTABLE_ID)
    parser.add_argument('--jumptable-histogram', action='store_true',
                        help='print the JumpTable-id histogram instead of per-file type counts')
    parser.add_argument('--all', action='store_true', help='with --root, scan every .tae found')
    parser.add_argument('--root', help='unpacked chr tree to search with --chr')
    parser.add_argument('--chr', nargs='*', default=[], help='chr ids to resolve under --root')
    parser.add_argument('--histogram', action='store_true', help='also print the event-type histogram')
    parser.add_argument('--selftest', action='store_true', help='parse one file and print a summary')
    parser.add_argument('--ability-report', action='store_true',
                        help='count every ability-producing TAE event type across the given files')
    parser.add_argument('--include-player', action='store_true',
                        help="with --ability-report, also count the player's c0000 TAEs")
    parser.add_argument('--arg-samples', type=int, default=0,
                        help='with --ability-report, print this many distinct resolved-id samples per type')
    args = parser.parse_args()

    paths = list(args.paths)
    if args.root and args.all:
        paths.extend(sorted(glob.glob(os.path.join(args.root, '**', '*.tae'), recursive=True)))
    elif args.root and args.chr:
        paths.extend(_tae_paths(args.root, args.chr))
    if args.ability_report:
        scanned, sites, chrs, arg_values, failures = ability_report(
            paths, skip_player=not args.include_player)
        print('scanned %d .tae files' % scanned)
        print('%-5s %-22s %-11s %8s %6s  %s' % ('type', 'handler', 'VA', 'sites', 'chrs', 'examples'))
        for event_type in sorted(ABILITY_EVENTS):
            name, va, index, _rule = ABILITY_EVENTS[event_type]
            examples = ' '.join(sorted(chrs[event_type])[:3]) or '-'
            print('%-5d %-22s 0x%-9x %8d %6d  %s'
                  % (event_type, name, va, sites[event_type], len(chrs[event_type]), examples))
        if args.arg_samples:
            for event_type in sorted(arg_values):
                common = arg_values[event_type].most_common(args.arg_samples)
                print('  type%-4d arg%d distinct=%d top=%s'
                      % (event_type, ABILITY_EVENTS[event_type][2],
                         len(arg_values[event_type]), common))
        for path, error in failures:
            print('FAIL %s: %s' % (path, error))
        return 1 if failures else 0
    if args.type is None and args.jumptable is None and not args.jumptable_histogram:
        args.jumptable = IFRAME_JUMPTABLE_ID
    if not paths:
        parser.error('no .tae paths given (pass files, or --root with --chr)')

    failures = 0
    scanned = 0
    jumptable_totals = collections.Counter()
    for path in paths:
        try:
            tae_id, animations = parse(path)
        except (BadTae, struct.error) as error:
            print('FAIL %s: %s' % (path, error))
            failures += 1
            continue
        total_events = sum(len(v) for v in animations.values())
        if args.jumptable_histogram:
            for events in animations.values():
                for event in events:
                    jid = jumptable_id(event)
                    if jid is not None:
                        jumptable_totals[jid] += 1
            scanned += 1
            continue
        if args.jumptable is not None:
            label = 'jt%d' % args.jumptable
            matching = sorted(a for a, v in animations.items()
                              if any(jumptable_id(e) == args.jumptable for e in v))
        else:
            label = 'type%d' % args.type
            matching = sorted(a for a, v in animations.items()
                              if any(e.type == args.type for e in v))
        scanned += 1
        if matching or not args.root:
            print('%-16s taeId=%-8d anims=%-5d events=%-6d %s_anims=%d %s'
                  % (os.path.basename(path), tae_id, len(animations), total_events,
                     label, len(matching), matching[:40]))
        if args.histogram or args.selftest:
            histogram = collections.Counter(e.type for v in animations.values() for e in v)
            print('    top event types:', histogram.most_common(15))
            print('    jumptable ids:',
                  collections.Counter(jumptable_id(e) for v in animations.values() for e in v
                                      if jumptable_id(e) is not None).most_common(15))
    if args.jumptable_histogram:
        print('scanned %d files; JumpTable id -> event count' % scanned)
        for jid in sorted(jumptable_totals):
            print('  jt%-4d %8d' % (jid, jumptable_totals[jid]))
    return 1 if failures else 0


if __name__ == '__main__':
    sys.exit(main())
