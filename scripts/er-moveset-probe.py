#!/usr/bin/env python3
"""Ad-hoc inspector for one creature's moveset join -- the thing you run when a row in
`crates/er-npc-possess/data/moveset.tbl` looks wrong and you need to see why.

Prints, per in-band animation: whether the graph makes it fireable, which animation it
actually plays, the TAE ability events it carries, and what each of those resolves to in
`BehaviorParam` / `AtkParam_Npc`. Read-only; touches nothing but the corpus and
regulation.bin.

  scripts/er-moveset-probe.py c4500
  scripts/er-moveset-probe.py c2120 --band 3000-3999
"""
import argparse
import importlib.util
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location(
    'gen', os.path.join(HERE, 'er-moveset-table-gen.py'))
GEN = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(GEN)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('chrid')
    parser.add_argument('--root', default=GEN.CORPUS_ROOT)
    parser.add_argument('--band', default='3000-4999')
    parser.add_argument('--regulation')
    options = parser.parse_args()
    low, high = (int(x) for x in options.band.split('-'))

    regulation = GEN.Regulation(options.regulation)
    variation = regulation.variation_for(options.chrid)
    behbnds = GEN.chr_dirs(options.root, 'behbnd').get(options.chrid)
    anibnds = GEN.chr_dirs(options.root, 'anibnd')
    if not behbnds:
        raise SystemExit(f'no behbnd for {options.chrid} under {options.root}')
    fireable, declared = GEN.fireable_animations(sorted(behbnds)[0])
    # Same join the generator uses, from the same function: a creature with no TimeAct of
    # its own reads its family base's. Duplicating the rule here instead would let the
    # probe say a creature has no attacks while the shipped table says it has thirty.
    tae_paths, tae_owner = GEN.tae_paths_for_chr(anibnds, options.chrid, variation)
    tae = {}
    for path in tae_paths:
        try:
            for anim, facts in GEN.tae_facts(path).items():
                tae.setdefault(anim, facts)
        except Exception as error:
            print(f'# {os.path.basename(path)}: {error!r}', file=sys.stderr)

    print(f'# {options.chrid} behaviorVariationId={variation} '
          f'fireable={len(fireable)} declared={len(declared)} tae={len(tae)} '
          f'tae_owner={tae_owner or "NONE"}',
          file=sys.stderr)
    print('anim\tfireable\tplays\tdur\tabilities\tresolved')
    for anim in sorted(set(fireable) | set(declared) | set(tae)):
        if not low <= anim <= high:
            continue
        played = fireable.get(anim, '-')
        facts = tae.get(played if isinstance(played, int) else anim, {})
        resolved = []
        for event_type, value in facts.get('abilities', []):
            row = regulation.resolve(event_type, value, variation)
            if row is None:
                resolved.append(f't{event_type}:{value}->MISS')
            else:
                atk = regulation.atk.get(row[1])
                resolved.append(f't{event_type}:{value}->ref{row[0]}:{row[1]}'
                                + (f'(dmg{atk[0]},r{atk[1]:.1f})' if atk else '(NOROW)'))
        print(f'{anim}\t{anim in fireable}\t{played}\t'
              f'{facts.get("duration", 0.0):.2f}\t'
              f'{len(facts.get("abilities", []))}\t{" ".join(resolved) or "-"}')


if __name__ == '__main__':
    main()
