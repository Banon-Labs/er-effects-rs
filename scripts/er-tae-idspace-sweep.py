#!/usr/bin/env python3
"""What id does the engine report for a playing animation, per creature?

`CSChrTimeActModule::animQueue[readIdx].animId` is filled by `TAE_Callback`, whose 1.16.2
signature names the argument **`taeId`** rather than `animId` -- so what a runtime reader
sees is the id of the TimeAct entry Havok is running, in whatever id space that creature's
TimeAct uses. `er-npc-possess` assumed one space ("below 3000 is neutral") and a Battlemage
(c3704) reported 43000 while standing still, which read as "permanently mid-attack" and
refused every press.

This answers the question offline, for every creature in the shipped moveset table: which
ids can that creature's TimeAct actually produce, and are the ids the table ships a subset
of them?

    python3 scripts/er-tae-idspace-sweep.py               # summary
    python3 scripts/er-tae-idspace-sweep.py --chr c3704   # one creature, verbose
    python3 scripts/er-tae-idspace-sweep.py --json out.json

Read-only over the unpacked corpus; override the root with ER_CHR_CORPUS_ROOT.
"""
import argparse
import collections
import importlib.util
import json
import multiprocessing
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def _load(name, filename):
    spec = importlib.util.spec_from_file_location(name, os.path.join(HERE, filename))
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


GEN = _load('er_moveset_table_gen', 'er-moveset-table-gen.py')
TAE = GEN.TAE

#: The table this crate ships, so the sweep covers exactly the creatures the mod can wear.
TABLE = os.path.join(os.path.dirname(HERE), 'crates', 'er-npc-possess', 'data', 'moveset.tbl')


def shipped_moves():
    """{chrId: {playedAnimId, ...}} straight out of the shipped table."""
    out = {}
    with open(TABLE, encoding='utf-8') as handle:
        for line in handle:
            line = line.strip()
            if not line or line.startswith('#') or line.startswith('v'):
                continue
            parts = line.split()
            if not parts[0].isdigit():
                continue
            played = set()
            for field in parts[1:]:
                if field.startswith('!') or field == '-':
                    continue
                head = field.split(':')[0]
                for marker in ('g', 'w'):
                    head = head.split(marker)[0]
                ids = head.split('=')
                played.add(int(ids[-1]))
            out[f'c{int(parts[0]):04d}'] = played
    return out


def _normalize_chr(value):
    """Accept `3704`, `c3704`, `C3704` or `0003704` alike -- all mean the same creature.

    The shipped table and every corpus dir key on the zero-padded `cNNNN` form (see
    `shipped_moves()` above), so a bare digit id silently misses every entry and prints
    `taeId(s)=[] anims=0` -- indistinguishable from a genuine corpus gap. That exact
    confusion cost a session two rounds guessing at a "missing" c3704 that was never
    missing; see bd `er-tae-idspace-sweep-chr-arg-bare-digits-look-like-corpus-gap-2026-09-02`.
    """
    digits = value[1:] if value[:1] in ('c', 'C') else value
    if digits.isdigit():
        return f'c{int(digits):04d}'
    return value


def _one(args):
    chr_id, paths = args
    ids = set()
    tae_ids = set()
    for path in paths:
        try:
            tae_id, anims = TAE.parse(path)
        except Exception:
            continue
        tae_ids.add(tae_id)
        # The generator collapses the `<group> * 1000000 + <id>` grouping the same way, so the
        # sweep and the shipped table are comparing the same numbers.
        ids |= {raw % 1000000 for raw in anims}
    return chr_id, sorted(ids), sorted(tae_ids)


def sweep(root, jobs):
    anibnds = GEN.chr_dirs(root, 'anibnd')
    regulation = GEN.Regulation(None)
    work = []
    for chr_id in sorted(shipped_moves()):
        variation = regulation.variation_for(chr_id)
        paths, _ = GEN.tae_paths_for_chr(anibnds, chr_id, variation)
        work.append((chr_id, paths))
    with multiprocessing.Pool(jobs) as pool:
        return dict((chr_id, (ids, tae_ids)) for chr_id, ids, tae_ids in pool.imap_unordered(_one, work))


#: What the live 2026-09-02 Battlemage run reported for a possessed c3704 -- an idle loop and
#: the two spawn-in clips. None of the three is in that creature's shipped moveset, whose ids
#: stop at 6023, so no threshold that still calls 3000 an attack can classify them.
BATTLEMAGE_OBSERVED = (43000, 3009000, 3009500)


def selftest():
    """The claim that justifies the runtime's positive test, checked without the corpus."""
    table = shipped_moves()
    if len(table) < 200:
        print(f'SELFTEST FAILED: only {len(table)} creatures parsed out of the shipped table')
        return 1
    battlemage = table.get('c3704')
    if not battlemage:
        print('SELFTEST FAILED: c3704 is not in the shipped table')
        return 1
    ceiling = max(battlemage)
    for observed in BATTLEMAGE_OBSERVED:
        if observed in battlemage:
            print(f'SELFTEST FAILED: {observed} is a shipped c3704 move after all')
            return 1
        if observed <= ceiling:
            print(f'SELFTEST FAILED: {observed} is below c3704 own top id {ceiling}')
            return 1
    # ...and the same statement for the whole table: every creature has ids the engine can play
    # that the table does not list, so "not in the table" must mean "not ours" rather than
    # "impossible".
    highest = max(max(ids) for ids in table.values() if ids)
    if highest >= min(BATTLEMAGE_OBSERVED):
        print(f'SELFTEST FAILED: the table ships {highest}, so a ceiling could have worked')
        return 1
    print(f'selftest ok: {len(table)} creatures, the table tops out at {highest}, and the three '
          f'ids a live Battlemage reported ({", ".join(map(str, BATTLEMAGE_OBSERVED))}) are all '
          f'above it -- no threshold separates them from an attack')
    return 0


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', default=GEN.CORPUS_ROOT)
    parser.add_argument('--chr', type=_normalize_chr,
                        help='accepts 3704, c3704 or C3704 -- all normalize to c3704')
    parser.add_argument('--json')
    parser.add_argument('--jobs', type=int, default=10)
    parser.add_argument('--selftest', action='store_true',
                        help='the corpus-free invariant; safe to run anywhere')
    options = parser.parse_args()

    if options.selftest:
        return selftest()

    table = shipped_moves()
    found = sweep(options.root, options.jobs)

    if options.chr:
        ids, tae_ids = found.get(options.chr, ([], []))
        print(f'{options.chr}: taeId(s)={tae_ids} anims={len(ids)}')
        bands = collections.Counter(anim // 1000 * 1000 for anim in ids)
        for band in sorted(bands):
            print(f'  {band:7d}-{band + 999:7d}: {bands[band]}')
        missing = sorted(table.get(options.chr, set()) - set(ids))
        print(f'  shipped moves not in its TimeAct: {missing}')
        return 0

    bands = collections.Counter()
    no_tae = []
    not_covered = {}
    for chr_id, (ids, _) in sorted(found.items()):
        if not ids:
            no_tae.append(chr_id)
            continue
        bands.update(anim // 1000 * 1000 for anim in ids)
        missing = table.get(chr_id, set()) - set(ids)
        if missing:
            not_covered[chr_id] = sorted(missing)

    print(f'creatures swept: {len(found)}   with no resolvable TimeAct: {len(no_tae)}')
    print('id bands present anywhere in the corpus (band: creatures using it):')
    for band in sorted(bands):
        print(f'  {band:7d}-{band + 999:7d}: {bands[band]}')
    print(f'\ncreatures whose shipped moves are NOT all in their own TimeAct: {len(not_covered)}')
    for chr_id, missing in sorted(not_covered.items())[:20]:
        print(f'  {chr_id}: {missing[:12]}')
    if no_tae:
        print(f'\nno resolvable TimeAct: {no_tae}')
    if options.json:
        with open(options.json, 'w', encoding='utf-8') as handle:
            json.dump({k: v[0] for k, v in found.items()}, handle)
        print(f'\nwrote {options.json}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
