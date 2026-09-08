#!/usr/bin/env python3
"""Prove `crates/er-npc-possess/data/moveset.tbl` is what the generator produces.

A shipped data table nobody can regenerate is a mystery blob: it cannot be reviewed, it cannot be
updated when the corpus changes, and a hand-edit in it is invisible. So this re-runs
`scripts/er-moveset-table-gen.py` over the local corpus and diffs the result against the committed
file, byte for byte.

Skips, loudly, when the corpus is not present -- the unpacked game assets are not in the repo and
never will be (no game-derived binaries), so CI and any machine without an extraction cannot run
the real check. Skipping is reported as a skip, never as a pass.

  scripts/check-moveset-table.py            # regenerate and diff, or skip
  scripts/check-moveset-table.py --selftest # prove the check catches a corrupted table
  scripts/check-moveset-table.py --shape    # grammar/invariant checks only; no corpus needed
"""
import argparse
import contextlib
import difflib
import importlib.util
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
GENERATOR = os.path.join(HERE, 'er-moveset-table-gen.py')
TABLE = os.path.join(ROOT, 'crates', 'er-npc-possess', 'data', 'moveset.tbl')

#: `<fired>[=<played>][w<chainCs>][g<victim>,<rangeDm>[+...]]:<bucket>:<rank>:<reach>[:<prefix>]`.
#: The prefix column is optional and absent means `W_Event`, the field-write path. The `g`
#: group is the throw spec: which victim chr id and range each matching `ThrowParam` row
#: demands. `g` on its own is rejected -- a grab with no row behind it is not a grab. The `w`
#: group is the chain window in centiseconds -- the start of the animation's TAE cancel window
#: -- and absent means the generator measured none, which the runtime reads as "committed for
#: the whole clip" rather than as zero.
ENTRY_RE = re.compile(
    r'^(\d+)(?:=(\d+))?(?:w(\d+))?(?:g(\d+,\d+(?:\+\d+,\d+)*))?'
    r':([0-3]):(\d+):([0-3])(?::(\d+))?$')
#: Reason codes are one or more digits: `throw-result-clip` is 10.
DENIAL_RE = re.compile(r'^!(\d+):([1-9]\d*)$')


_GEN = None


def GEN_MODULE():
    """The generator, loaded once. `multiprocessing` inside it needs the module to be
    importable by the name it was loaded under, so the name is registered in
    `sys.modules` -- without that the worker pool cannot pickle `_one`."""
    global _GEN
    if _GEN is None:
        spec = importlib.util.spec_from_file_location('er_moveset_table_gen', GENERATOR)
        module = importlib.util.module_from_spec(spec)
        sys.modules['er_moveset_table_gen'] = module
        spec.loader.exec_module(module)
        _GEN = module
    return _GEN


def check_shape(text):
    """Grammar and invariants that need no corpus, so they run everywhere."""
    problems = []
    version_seen = False
    creatures = 0
    moves = 0
    grabs = 0
    windows = 0
    for number, line in enumerate(text.splitlines(), 1):
        line = line.strip()
        if not line or line.startswith('#'):
            continue
        if line.startswith('v'):
            version_seen = True
            continue
        parts = line.split()
        if len(parts) < 2 or not parts[0].isdigit():
            problems.append(f'line {number}: no chr id')
            continue
        creatures += 1
        if parts[1:] == ['-']:
            continue
        ranks = {}
        offered = set()
        denied = set()
        for field in parts[1:]:
            if field.startswith('!'):
                match = DENIAL_RE.match(field)
                if not match:
                    problems.append(f'line {number}: bad denial {field!r}')
                    continue
                denied.add(int(match.group(1)))
                continue
            match = ENTRY_RE.match(field)
            if not match:
                problems.append(f'line {number}: bad entry {field!r}')
                continue
            moves += 1
            fired, bucket, rank = int(match.group(1)), int(match.group(5)), int(match.group(6))
            prefix = int(match.group(8) or 0)
            if match.group(3) is not None:
                windows += 1
                # A zero window would mean "chainable from the first frame", i.e. cancellable
                # before the swing has begun, which is the behaviour this column exists to
                # stop. The generator writes no `w` at all when it measured nothing, so a
                # literal `w0` can only be a rounding or units mistake.
                if int(match.group(3)) == 0:
                    problems.append(
                        f'line {number}: entry {field!r} declares a chain window of zero; '
                        f'omit the window instead of claiming the move is cancellable on its '
                        f'first frame')
            if match.group(4):
                grabs += 1
                # The grab is the INITIATOR, never the throw-result clip. If a 4000-band id
                # ever shows up marked as a grab, the join has gone back to reading TimeAct
                # event 304 -- which is the mistake that made `allow_grabs` gate nothing.
                lo, hi = GEN_MODULE().ATTACK_BAND
                if not lo <= fired <= hi:
                    problems.append(
                        f'line {number}: entry {field!r} is marked a grab but sits outside '
                        f'the attack band {lo}-{hi}')
                for row in match.group(4).split('+'):
                    if int(row.split(',')[1]) <= 0:
                        problems.append(
                            f'line {number}: entry {field!r} has a zero-range ThrowParam row')
            if prefix >= len(GEN_MODULE().PREFIXES):
                problems.append(
                    f'line {number}: entry {field!r} names prefix {prefix}, and the generator '
                    f'only has {len(GEN_MODULE().PREFIXES)}')
            offered.add(fired)
            ranks.setdefault(bucket, []).append(rank)
        both = offered & denied
        if both:
            problems.append(f'line {number}: {sorted(both)} are both offered and denied')
        for bucket, values in ranks.items():
            if sorted(values) != list(range(len(values))):
                problems.append(
                    f'line {number}: bucket {bucket} ranks are not a dense 0..n: {sorted(values)}')
    if not version_seen:
        problems.append('no version marker')
    if creatures < 200:
        problems.append(f'only {creatures} creatures')
    if moves < 2000:
        problems.append(f'only {moves} moves')
    # The regression this file exists to catch. The table shipped with zero grab-marked moves
    # for a whole layer, because the marker was on the throw-result clip (which nothing can
    # fire) instead of on the initiator. Zero here is not "this creature has no grabs"; it is
    # "the ThrowParam join is not running", and it makes `allow_grabs` a dead setting again.
    if grabs < 100:
        problems.append(
            f'only {grabs} grab-marked moves -- the corpus has 153 across 78 creatures; the '
            'AtkParam_Npc.throwTypeId -> ThrowParam join is not producing them')
    # The same shape of regression, one column along. The chain window is a TAE type-0 event
    # with FlagType 86 -- the creature cancel-into-attack flag. FlagType 4 is the player one and
    # covers 0.3% of non-player attack animations, so reading the wrong number would leave this
    # near zero while everything still parsed. Near zero is not "these creatures cannot combo";
    # it is "the runtime will make the player wait out every animation".
    if windows < moves // 2:
        problems.append(
            f'only {windows} of {moves} moves carry a chain window -- most attack animations '
            'author a TAE type-0 ChrActionFlag event with FlagType 86, so this low a count means '
            'the generator is reading the wrong flag (4 is the player one) or the wrong param')
    return problems


def corpus_present(root):
    return bool(GEN_MODULE().chr_dirs(root, 'behbnd'))


def regenerate(root, jobs):
    """Rebuild the whole table in this process and return the text.

    In-process rather than `subprocess.run(GENERATOR)` for two reasons. The repo bans a
    Python subprocess without a timeout of 30 seconds or less
    (`scripts/check-no-timeouts.py`, enforced by the pre-commit hook), and a full-corpus
    regeneration takes minutes -- so there is no timeout that is both legal and true. And
    calling the generator's own functions is the stronger check anyway: it compares the
    committed file against what this tree's generator produces, with no room for a stale
    interpreter or a different working directory to change the answer.
    """
    # The generator narrates one line per creature on stderr, which is useful when a
    # human runs it and pure noise inside a gate whose own verdict is one line.
    with open(os.devnull, 'w', encoding='utf-8') as quiet:
        with contextlib.redirect_stderr(quiet):
            table, failures = GEN_MODULE().generate(root, None, jobs, None)
    if failures:
        raise SystemExit(
            'the generator could not read {} creature(s): {}'.format(
                len(failures), sorted(failures)[:5]))
    return GEN_MODULE().format_table(table)


def selftest():
    """The check must fail on a table that does not match its own grammar."""
    corruptions = [
        ('4500 3000:0:0:1 3000:0:1:1 !3000:3', 'offered and denied'),
        ('4500 6000:3:0:0:99', 'a prefix index the generator does not have'),
        ('4500 3000:0:0:1 3001:0:2:1', 'rank gap'),
        ('4500 3000:9:0:1', 'bucket out of range'),
        ('4500 nonsense', 'unparsable entry'),
        # The grab REGRESSIONS. A bare `g` is the pre-ThrowParam spelling and must not parse;
        # a grab on a 4000-band id means the marker went back onto the throw-result clip.
        ('4500 3000g:0:0:1', 'a grab with no ThrowParam row behind it'),
        ('4500 4100g0,100:0:0:1', 'a grab marked on a throw-result clip'),
        ('4500 3000g0,0:0:0:1', 'a zero-range ThrowParam row'),
        # A zero chain window is not the same statement as no window: it claims the attack may
        # be cancelled on its first frame, which is the behaviour the column exists to prevent.
        ('4500 3000w0:0:0:1', 'a chain window of zero'),
    ]
    for corrupt, why in corruptions:
        text = 'v1\n' + (corrupt + '\n') * 250
        if not check_shape(text):
            print(f'SELFTEST FAILED: shape check accepted {why}: {corrupt!r}')
            return 1
    # ...and pass on a well-formed one.
    # Exercises every part of the grammar at once: dense ranks in two buckets, the
    # plays-something-else spelling, both grab spellings (one victim and two), a two-digit
    # denial reason, and a plain denial.
    good = 'v1\n' + '\n'.join(
        f'{4000 + n} '
        + ' '.join(f'{3000 + rank}w{100 + rank}:0:{rank}:1' for rank in range(5))
        + ' 3100w250:1:0:2 3101w300g0,100:1:1:1 3102w310g0,100+3300,55:1:2:1'
        + ' 3110=3000:2:0:3 6000:3:0:0:2 !3010:3 !4100:10'
        for n in range(300))
    problems = check_shape(good + '\n')
    if problems:
        print(f'SELFTEST FAILED: shape check rejected a good table: {problems[:3]}')
        return 1
    print(f'selftest ok: the shape check catches all {len(corruptions)} corruptions and accepts '
          'a good table')
    return 0


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', default=None,
                        help='unpacked chr corpus root (default: ER_CHR_CORPUS_ROOT or the '
                             'generator default)')
    parser.add_argument('--shape', action='store_true',
                        help='grammar/invariant checks only; never needs the corpus')
    parser.add_argument('--selftest', action='store_true')
    parser.add_argument('--jobs', type=int,
                        default=int(os.environ.get('SWEEP_JOBS', os.cpu_count() or 8)),
                        help='parallel workers for the regeneration')
    options = parser.parse_args()
    if options.selftest:
        return selftest()

    if not os.path.exists(TABLE):
        print(f'FAIL: {TABLE} is missing')
        return 1
    with open(TABLE, encoding='utf-8') as handle:
        committed = handle.read()

    problems = check_shape(committed)
    if problems:
        print('FAIL: the committed table does not satisfy its own grammar:')
        for problem in problems[:20]:
            print(f'  {problem}')
        return 1
    print(f'shape ok: {len(committed)} bytes')
    if options.shape:
        return 0

    root = options.root or GEN_MODULE().CORPUS_ROOT
    if not corpus_present(root):
        print(f'SKIP: no unpacked chr corpus at {root}, so the table cannot be regenerated here. '
              'Set ER_CHR_CORPUS_ROOT or pass --root. The shape checks above did run.')
        return 0

    regenerated = regenerate(root, options.jobs)
    if regenerated == committed:
        print('regenerates identically')
        return 0
    print('FAIL: the committed table is not what the generator produces from this corpus.')
    diff = difflib.unified_diff(
        committed.splitlines(), regenerated.splitlines(),
        'committed', 'regenerated', lineterm='', n=0)
    for line in list(diff)[:40]:
        print(f'  {line[:200]}')
    print('  Regenerate with: scripts/er-moveset-table-gen.py --out '
          'crates/er-npc-possess/data/moveset.tbl')
    return 1


if __name__ == '__main__':
    sys.exit(main())
