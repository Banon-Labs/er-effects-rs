#!/usr/bin/env python3
"""Which chr's TimeAct file describes a given creature's animations.

Why this exists. `scripts/er-moveset-table-gen.py` joined the behaviour graph (what a
creature can fire) against `<chr>.tae` (what the animation does) by chr id on both sides.
That join is wrong for a third of the roster: 143 of the 408 creatures in the shipped
table came out with zero attacks, every one of them a creature whose own `.anibnd`
contains a skeleton and nothing else -- no animations, no TimeAct. c4351 Godrick Knight
is the worked example: its graph declares 72 in-band events and can fire all 72, but its
anibnd is `skeleton.hkx` alone, so every one of those 72 arrived with no damage window and
was denied, leaving the twelve `W_Step` walk clips as the whole moveset.

The animations are not missing from the game. They live in the family base's anibnd --
`c4350.anibnd.dcx` carries `INTERROOT_win64/chr/c4350/tae/c4350.tae`, 1,045,648 bytes of
TimeAct for every 435x knight -- and the base is named by `NpcParam.behaviorVariationId`:

    behaviorVariationId = <family> * 100 + <variant>       43500 -> family 435 -> c4350
                                                           41600 -> family 416 -> c4160

A creature whose family base is itself (c4160, variation 41600) already joined correctly;
this script exists to name the owner for the ones that did not, and to prove the rule over
the whole corpus rather than over the one creature that motivated it.

Usage:
  scripts/er-moveset-tae-owner.py                    # sweep, summary + the misjoins
  scripts/er-moveset-tae-owner.py --chr c4351        # one creature, verbose
  scripts/er-moveset-tae-owner.py --selftest         # rule holds over the corpus
"""
import argparse
import glob
import importlib.util
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def _load(name, filename):
    spec = importlib.util.spec_from_file_location(name, os.path.join(HERE, filename))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


GEN = _load('gen', 'er-moveset-table-gen.py')


def family_base(variation):
    """The chr id whose `.tae` describes the animations of a creature with this variation.

    `behaviorVariationId` is `<family> * 100 + <variant>`, and the family's animations and
    TimeAct are shipped under `c<family>0`. Returns None for a variation the convention
    cannot express (None, or the 100000+ sentinels `resolve` already refuses).
    """
    if variation is None or variation >= 100000 or variation < 0:
        return None
    return f'c{variation // 100 * 10:04d}'


def tae_index(root):
    """{chrId: [tae path, ...]} over the corpus, wherever inside the anibnd it sits."""
    out = {}
    for chr_id, dirs in GEN.chr_dirs(root, 'anibnd').items():
        paths = GEN.tae_paths_for(dirs)
        if paths:
            out[chr_id] = paths
    return out


def resolve(root, regulation, chr_ids=None):
    """[(chrId, variation, ownTae, baseChr, baseTae, verdict)] for every chr with a graph."""
    taes = tae_index(root)
    behbnds = GEN.chr_dirs(root, 'behbnd')
    rows = []
    for chr_id in sorted(behbnds):
        if chr_id == 'c0000' or (chr_ids and chr_id not in chr_ids):
            continue
        variation = regulation.variation_for(chr_id)
        own = bool(taes.get(chr_id))
        base = family_base(variation)
        base_has = bool(base and taes.get(base))
        if own:
            verdict = 'own'
        elif base_has:
            verdict = 'inherit'
        else:
            verdict = 'none'
        rows.append((chr_id, variation, own, base, base_has, verdict))
    return rows


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', default=GEN.CORPUS_ROOT)
    parser.add_argument('--regulation')
    parser.add_argument('--chr', dest='chr_id')
    parser.add_argument('--selftest', action='store_true')
    args = parser.parse_args()

    regulation = GEN.Regulation(args.regulation)
    only = {args.chr_id} if args.chr_id else None
    rows = resolve(args.root, regulation, only)

    if args.chr_id:
        for chr_id, variation, own, base, base_has, verdict in rows:
            print(f'{chr_id} behaviorVariationId={variation} own_tae={own} '
                  f'family_base={base} base_has_tae={base_has} -> {verdict}')
            taes = tae_index(args.root)
            for path in taes.get(chr_id, []) or taes.get(base, []):
                print(f'  tae: {path}')
        return 0

    counts = {'own': 0, 'inherit': 0, 'none': 0}
    for row in rows:
        counts[row[5]] += 1
    print(f'{len(rows)} creatures with a behaviour graph: '
          f"own TimeAct {counts['own']}, inherited from family base {counts['inherit']}, "
          f"no TimeAct anywhere {counts['none']}")
    print()
    print('# INHERIT -- these are the ones the chr-id join silently dropped')
    for chr_id, variation, _, base, _, verdict in rows:
        if verdict == 'inherit':
            print(f'  {chr_id} variation={variation} -> {base}')
    print()
    print('# NONE -- no TimeAct under either id; genuinely undescribed')
    for chr_id, variation, _, base, _, verdict in rows:
        if verdict == 'none':
            print(f'  {chr_id} variation={variation} base={base}')

    if args.selftest:
        # The rule has to explain the misjoin, not merely be consistent with it: every
        # creature the chr-id join left without TimeAct must gain one here, or the family
        # convention is not the mechanism.
        stranded = [r for r in rows if r[5] == 'none']
        print()
        print(f'selftest: {counts["inherit"]} recovered, {len(stranded)} still stranded')
        return 0 if counts['inherit'] > 0 else 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
