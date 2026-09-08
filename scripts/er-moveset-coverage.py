#!/usr/bin/env python3
"""Audit the shipped moveset table for creatures no attack button can reach.

The table's job is to say what a possessed creature can do. A row that classifies every
move into the `movement` bucket says the creature can walk and nothing else, and with the
default `r1=light r2=heavy l1=ranged l2=movement` mapping that leaves three of the four
attack buttons dead and the fourth playing a walk cycle. That is either the truth (a
Balloon Dummy has no attacks) or a hole in the generator, and the two are indistinguishable
from the table alone -- which is what this script is for.

It joins the shipped table against `scripts/er-moveset-tae-owner.py`, so every attackless
creature comes out labelled with a CAUSE:

  genuinely-attackless   its own TimeAct was read and describes no attack
  missing-timeact        its TimeAct lives under the family base and was not read
  no-timeact-anywhere    no TimeAct under either id in this corpus

Usage:
  scripts/er-moveset-coverage.py                          # summary + attackless list
  scripts/er-moveset-coverage.py --table <path> --json out.json
  scripts/er-moveset-coverage.py --check                  # exit 1 if any missing-timeact
"""
import argparse
import collections
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_TABLE = os.path.join(HERE, '..', 'crates', 'er-npc-possess', 'data', 'moveset.tbl')
DEFAULT_NAMES = os.path.join(HERE, '..', 'crates', 'er-npc-possess', 'data', 'chrnames.tbl')

BUCKET_NAMES = ('light', 'heavy', 'ranged', 'movement')
ENTRY = re.compile(r'^(\d+)(?:=(\d+))?(?:w(\d+))?(?:g[\d,+]+)?:(\d):(\d+):(\d)(?::(\d+))?$')


def parse_table(path):
    """{chrNum: {'buckets': Counter, 'denials': Counter, 'empty': bool}}"""
    out = {}
    for line in open(path, encoding='utf-8'):
        line = line.rstrip('\n')
        if not line or line.startswith('#') or line.startswith('v'):
            continue
        head, _, rest = line.partition(' ')
        if not head.isdigit():
            continue
        buckets, denials, empty = collections.Counter(), collections.Counter(), False
        for token in rest.split():
            if token == '-':
                empty = True
            elif token.startswith('!'):
                denials[int(token.rsplit(':', 1)[1])] += 1
            else:
                match = ENTRY.match(token)
                if not match:
                    raise SystemExit(f'unparsable entry on chr {head}: {token!r}')
                buckets[int(match.group(4))] += 1
        out[int(head)] = {'buckets': buckets, 'denials': denials, 'empty': empty}
    return out


def parse_names(path):
    names = {}
    for line in open(path, encoding='utf-8'):
        if line.startswith('#') or not line.strip() or line.startswith('v'):
            continue
        parts = line.rstrip('\n').split('\t')
        if len(parts) == 2:
            names[int(parts[0])] = parts[1]
    return names


def causes(attackless, root=None, regulation=None):
    """{chrNum: cause} for the creatures that classified no attacks.

    The oracle is the TimeAct itself, not the count in the table. A creature with zero attacks is
    either a Balloon Dummy or a Godrick Knight whose animations were not read, and the table cannot
    tell you which -- both rows look like twelve `W_Step` clips and `denied=0`. So this opens the
    TimeAct that describes the creature (its own, or its family base's; see
    `er-moveset-tae-owner.py`) and counts attack-band animations carrying an ability event that
    resolves to a real `AtkParam_Npc` or `Bullet` row. Non-zero means the game has attacks for this
    creature that the shipped table is not offering, which is a bug; zero means it genuinely has
    none.

    Returns `{}` when the corpus is absent, so the audit still runs anywhere -- as a shape report
    with no causes, never as a pass.
    """
    import importlib.util
    spec = importlib.util.spec_from_file_location(
        'taeowner', os.path.join(HERE, 'er-moveset-tae-owner.py'))
    module = importlib.util.module_from_spec(spec)
    try:
        spec.loader.exec_module(module)
        gen = module.GEN
        reg = gen.Regulation(regulation)
        corpus = root or gen.CORPUS_ROOT
        anibnds = gen.chr_dirs(corpus, 'anibnd')
    except Exception as error:                         # no corpus / no regulation here
        print(f'# corpus unavailable, cause column omitted: {error!r}', file=sys.stderr)
        return {}
    if not anibnds:
        # An empty corpus must not look like an answer. `chr_dirs` globs and returns `{}` for a
        # path that is not there rather than raising, so without this every creature would come
        # back "no TimeAct anywhere" and the gate would report a clean pass on a machine with no
        # game files at all -- the single worst outcome for a check whose whole job is to notice
        # missing TimeAct.
        print(f'# no anibnd directories under {corpus}, cause column omitted', file=sys.stderr)
        return {}

    out = {}
    for chr_num in attackless:
        chr_id = f'c{chr_num:04d}'
        variation = reg.variation_for(chr_id)
        paths, owner = gen.tae_paths_for_chr(anibnds, chr_id, variation)
        if not paths:
            out[chr_num] = 'no-timeact-anywhere'
            continue
        described = 0
        low, high = gen.ATTACK_BAND
        for path in paths:
            try:
                facts = gen.tae_facts(path)
            except Exception:
                continue
            for anim, fact in facts.items():
                if not low <= anim <= high:
                    continue
                if any(reg.resolve(kind, value, variation) is not None
                       for kind, value in fact.get('abilities', ())):
                    described += 1
        out[chr_num] = (f'ATTACKS-NOT-OFFERED ({described} in {owner}.tae)' if described
                        else 'genuinely-attackless')
    return out


#: The creature the selftest corrupts. c4351 Godrick Knight is the one that motivated this gate:
#: its own anibnd is a skeleton, its animations and TimeAct ship under c4350, and before the family
#: join it came out of the generator with twelve walk clips and nothing else.
SELFTEST_CHR = 4351
SELFTEST_MOVEMENT_ONLY = ' '.join(f'{6000 + n}:3:{n}:0:2' for n in range(4))


def selftest(args):
    """Prove the gate catches the failure it exists for, rather than only agreeing with today.

    Rewrites one creature's row to the movement-only shape the broken join produced and asserts the
    audit calls it out. A gate whose only evidence is "it passes on the current table" cannot tell
    a fix from a check that has quietly stopped looking.
    """
    import tempfile
    if args.no_corpus or not causes([SELFTEST_CHR], args.root, args.regulation):
        # Skips loudly, like the gate it is testing. Without an extraction there is no TimeAct to
        # read, so the corrupted row would be indistinguishable from a genuinely attackless one and
        # a "failure" here would say nothing about the check.
        print('selftest SKIPPED: no corpus, so the cause oracle cannot run -- not a pass',
              file=sys.stderr)
        return 0
    text = open(args.table, encoding='utf-8').read()
    lines = text.splitlines(keepends=True)
    for index, line in enumerate(lines):
        if line.startswith(f'{SELFTEST_CHR} '):
            lines[index] = f'{SELFTEST_CHR} {SELFTEST_MOVEMENT_ONLY}\n'
            break
    else:
        raise SystemExit(f'selftest: c{SELFTEST_CHR} is not in {args.table}')
    with tempfile.NamedTemporaryFile('w', suffix='.tbl', delete=False,
                                     encoding='utf-8') as handle:
        handle.writelines(lines)
        corrupted = handle.name
    try:
        broken = argparse.Namespace(**vars(args))
        broken.table = corrupted
        broken.selftest = False
        broken.check = True
        broken.json = None
        code = run(broken)
    finally:
        os.unlink(corrupted)
    if code == 0:
        raise SystemExit(
            'selftest FAILED: a movement-only row for a creature whose TimeAct describes attacks '
            'was accepted. Either the corpus is absent (this is a skip, not a pass) or the cause '
            'oracle has stopped reading TimeAct.')
    print(f'selftest ok: c{SELFTEST_CHR} reduced to locomotion is caught')
    return 0


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--table', default=DEFAULT_TABLE)
    parser.add_argument('--names', default=DEFAULT_NAMES)
    parser.add_argument('--root')
    parser.add_argument('--regulation')
    parser.add_argument('--json')
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--no-corpus', action='store_true',
                        help='skip the cause column; table-only audit')
    parser.add_argument('--selftest', action='store_true',
                        help='prove --check catches a creature reduced to locomotion')
    args = parser.parse_args()
    if args.selftest:
        return selftest(args)
    return run(args)


def run(args):
    table = parse_table(args.table)
    names = parse_names(args.names)

    report, attackless = [], []
    for chr_num, row in sorted(table.items()):
        attacks = sum(row['buckets'][b] for b in (0, 1, 2))
        entry = {
            'chr': chr_num,
            'name': names.get(chr_num, '-'),
            'attacks': attacks,
            'movement': row['buckets'][3],
            'denied': sum(row['denials'].values()),
            'empty': row['empty'],
            'cause': 'unknown',
        }
        report.append(entry)
        if attacks == 0:
            attackless.append(entry)

    cause = {} if args.no_corpus else causes([e['chr'] for e in attackless],
                                             args.root, args.regulation)
    for entry in attackless:
        entry['cause'] = cause.get(entry['chr'], 'unknown')

    by_cause = collections.Counter(e['cause'].split(' (')[0] for e in attackless)
    print(f'{len(table)} creatures in {os.path.relpath(args.table)}; '
          f'{len(attackless)} classify ZERO attacks')
    for label, count in sorted(by_cause.items()):
        print(f'  {label:<22} {count}')
    print()
    for entry in attackless:
        flag = 'BUG' if entry['cause'].startswith('ATTACKS-NOT-OFFERED') else '   '
        print(f"{flag} c{entry['chr']:<5} {entry['name']:<38} "
              f"moves={entry['movement']:<3} denied={entry['denied']:<3} {entry['cause']}")

    if args.json:
        with open(args.json, 'w', encoding='utf-8') as handle:
            json.dump(report, handle, indent=1)

    if args.check:
        # Skipping is not passing, and both ways of ending up without causes say so out loud: the
        # corpus is game-derived and will never be in the repo, so this gate is honest about being
        # unable to run rather than reporting a pass it did not earn. An empty `cause` with an
        # empty `attackless` is the real pass -- nothing to explain.
        if args.no_corpus or (attackless and not cause):
            print('\nSKIP: no corpus, so nothing was checked -- this is not a pass',
                  file=sys.stderr)
            return 0
        broken = [e for e in attackless if e['cause'].startswith('ATTACKS-NOT-OFFERED')]
        if broken:
            print(f'\nFAIL: {len(broken)} creatures have TimeAct-described attacks the shipped '
                  f'table does not offer', file=sys.stderr)
            return 1
        print('\nOK: every attackless creature in the table has no TimeAct-described attack '
              'under its own id or its family base')
    return 0


if __name__ == '__main__':
    sys.exit(main())
