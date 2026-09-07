#!/usr/bin/env python3
"""Generate the shipped `er-npc-possess` creature-name table.

`data/moveset.tbl` is integers only and says so in its own header: it is a set of
decisions about animation ids, and an animation id is a BND entry filename. That was
enough for the moveset layer, which never had to put a creature in front of a person.
A PICKER does. `c4630` is not a thing anybody can choose from a list of four hundred;
`Runebear` is.

Where the names come from, and why it is not the game'S own strings. The obvious
source is `NpcName.fmg` out of `item.msgbnd`, and it is the wrong one: it holds the
strings the game displays, which means bosses and named NPCs and nothing else. Measured
over the 408 creatures the moveset table covers, `NpcName.fmg` reaches 73. Routing
`NpcParam.nameId` into it reaches 30. The overwhelming majority of the creatures a
player would want to wear -- every wolf, every soldier, every knight -- have no
displayable name in the game at all, because the game never shows one.

So the names here are the PARAMDEX/SMITHBOX row NAMES: community-authored labels for
param rows, which exist precisely because the rows themselves are anonymous. Two
files, in priority order:

  ChrModelParam   row id is the chr id. 256/408, and they are the clean ones --
                  "Flying Dragon", not "Flying Dragon Agheel (Limgrave)".
  NpcParam        row id is chrid*10000 + variant. 405/408, with location and
                  scaling qualifiers attached. Used only where ChrModelParam is blank.

Combined: 405 of 408. The three that neither names (5194, 5261, 6240) ship as `-`,
and the picker shows `c5194` for them rather than inventing a name.

This is not a game asset and is not a game byte. Nothing here is read out of
`regulation.bin`, out of a `.dcx`, or out of the running game. The input is a JSON file
of English labels a community wrote to make an anonymous table navigable, and the
output is the same labels keyed by the ids this crate already ships. `regulation.bin`
is not opened by this script at all.

Usage:
  scripts/er-chr-names-gen.py --out crates/er-npc-possess/data/chrnames.tbl
  scripts/er-chr-names-gen.py --selftest       # no corpus needed
  scripts/er-chr-names-gen.py --report         # coverage, to stdout, writes nothing

Override the row-name checkout with ER_PARAM_ROWNAME_DIR, the same variable
`scripts/er-param-read.py` reads.
"""
import argparse
import collections
import json
import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
_REPO = os.path.dirname(_HERE)

ROWNAME_DIR = os.environ.get(
    'ER_PARAM_ROWNAME_DIR',
    os.path.expanduser(
        '~/.local/share/smithbox/app/Assets/PARAM/ER/Param Row Names/English'))

MOVESET_TBL = os.path.join(_REPO, 'crates', 'er-npc-possess', 'data', 'moveset.tbl')
DEFAULT_OUT = os.path.join(_REPO, 'crates', 'er-npc-possess', 'data', 'chrnames.tbl')

TABLE_VERSION = 1

# A name is one line of a list a person reads at a glance. Longer than this and it
# stops being a label; the longest real one is 49 characters
# ("God-Devouring Serpent / Rykard, Lord of Blasphemy") and it is a slash-joined pair
# rather than one name. The cap is enforced here rather than at the draw so the
# generated file is the thing that got measured.
NAME_MAX_CHARS = 56

# `-` rather than an empty field, matching moveset.tbl's own spelling for "considered,
# and there is nothing". An absent line would mean something different -- an id this
# generator never looked at -- and the two must not be confusable.
NO_NAME = '-'


def moveset_chr_ids(path):
    """Every chr id the shipped moveset table covers, in file order."""
    ids = []
    with open(path, encoding='utf-8') as handle:
        for line in handle:
            line = line.strip()
            if not line or line.startswith('#'):
                continue
            head = line.split(None, 1)[0]
            if not head.isdigit():
                # The `v2` version marker and anything else non-numeric.
                continue
            ids.append(int(head))
    return ids


def load_row_names(stem, directory):
    """`{row id: [label, ...]}` from one Smithbox row-name JSON.

    The schema is `{"Name": "<param>", "Entries": [{"ID": n, "Entries": ["label"]}]}`
    -- note the two different `Entries`. `scripts/er-param-read.py` read the inner one
    as `e["Name"]` and so returned `None` for every row in the file; that is fixed
    there, and this reads the real shape.
    """
    path = os.path.join(directory, stem + '.json')
    if not os.path.exists(path):
        return None
    with open(path, encoding='utf-8-sig') as handle:
        doc = json.load(handle)
    rows = doc['Entries'] if isinstance(doc, dict) else doc
    out = {}
    for row in rows:
        labels = [' '.join(str(x).split()) for x in (row.get('Entries') or [])]
        labels = [label for label in labels if label and label != NO_NAME]
        if labels:
            out[int(row['ID'])] = labels
    return out


def choose_npc_name(labels):
    """One label out of every NpcParam variant of a creature.

    Deterministic and stated rather than "the first one": most frequent wins, because
    the plain name repeats across every map placement while a qualified one
    ("Flying Dragon Agheel (Limgrave)") appears once; ties go to the shortest, which
    is the one without the qualifier; remaining ties go alphabetically so a
    regeneration on another machine produces the same file.
    """
    counts = collections.Counter(labels)
    return min(counts, key=lambda name: (-counts[name], len(name), name))


def build(chr_ids, chr_model, npc):
    """`[(chr_id, name, source)]`, one per id, in ascending id order."""
    rows = []
    for chr_id in sorted(chr_ids):
        direct = chr_model.get(chr_id)
        if direct:
            rows.append((chr_id, direct[0][:NAME_MAX_CHARS], 'ChrModelParam'))
            continue
        variants = []
        for row_id, labels in npc.items():
            if row_id // 10000 == chr_id:
                variants.extend(labels)
        if variants:
            rows.append((chr_id, choose_npc_name(variants)[:NAME_MAX_CHARS], 'NpcParam'))
            continue
        rows.append((chr_id, NO_NAME, 'none'))
    return rows


def render(rows):
    named = sum(1 for _, name, _ in rows if name != NO_NAME)
    by_source = collections.Counter(source for _, _, source in rows)
    lines = [
        '# er-npc-possess creature-name table -- GENERATED, do not hand-edit.',
        '# Regenerate: scripts/er-chr-names-gen.py --out '
        'crates/er-npc-possess/data/chrnames.tbl',
        '#',
        '# One line per creature:  <chrid> <tab> <name>',
        '# `-` means no source names this id; the picker shows cNNNN for those.',
        '#',
        '# The names are Paramdex/Smithbox PARAM ROW NAMES -- community-authored labels',
        '# for anonymous param rows -- not strings out of the game. The game\'s own',
        '# NpcName.fmg holds only bosses and named NPCs and reaches 73 of these 408; a',
        '# wolf has no displayable name because the game never shows one. See the',
        '# generator\'s docstring for the two files and the priority between them.',
        '#',
        f'# {named} of {len(rows)} named: '
        + ', '.join(f'{count} from {source}' for source, count in sorted(by_source.items())),
        f'v{TABLE_VERSION}',
    ]
    for chr_id, name, _ in rows:
        lines.append(f'{chr_id}\t{name}')
    return '\n'.join(lines) + '\n'


def selftest():
    """Everything that can be checked without the row-name checkout."""
    assert choose_npc_name(['Wolf', 'Wolf', 'Wolf (Limgrave)']) == 'Wolf'
    # Frequency beats length: the qualified name is not shorter, but if it repeated
    # more it would still lose to nothing else here.
    assert choose_npc_name(['A (x)', 'A (x)', 'B']) == 'A (x)'
    # Length breaks a frequency tie.
    assert choose_npc_name(['Runebear', 'Runebear (Mistwood)']) == 'Runebear'
    # Shorter wins before the alphabet does: 'Beta' beats 'Alpha' on length.
    assert choose_npc_name(['Beta', 'Alpha']) == 'Beta'
    # And the alphabet breaks what length cannot, so two machines agree.
    assert choose_npc_name(['Bear', 'Ants']) == 'Ants'

    rows = build([7, 9], {7: ['Seven']}, {90000: ['Nine'], 90001: ['Nine']})
    assert rows == [(7, 'Seven', 'ChrModelParam'), (9, 'Nine', 'NpcParam')], rows
    rows = build([5], {}, {})
    assert rows == [(5, NO_NAME, 'none')], rows

    # The `v2` marker in moveset.tbl is not a chr id, and a comment is not one either.
    import tempfile
    with tempfile.NamedTemporaryFile('w', suffix='.tbl', delete=False) as handle:
        handle.write('# comment\nv2\n100 6000:3:0:0:2\n2010 -\n')
        tmp = handle.name
    try:
        assert moveset_chr_ids(tmp) == [100, 2010], moveset_chr_ids(tmp)
    finally:
        os.unlink(tmp)

    text = render([(1, 'One', 'ChrModelParam'), (2, NO_NAME, 'none')])
    assert text.endswith('1\tOne\n2\t-\n'), repr(text[-40:])
    assert f'v{TABLE_VERSION}\n' in text
    # The reader itself, against a real file. The bug this whole module documents -- reading
    # `e['Name']` where the schema nests a second `Entries` -- lives in `load_row_names`, so a
    # selftest that re-implements one of its lines inline would pass with that bug restored. It
    # did, until this used a temp directory.
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        with open(os.path.join(tmp, 'ChrModelParam.json'), 'w', encoding='utf-8') as handle:
            handle.write('{"Name":"ChrModelParam","Entries":['
                         '{"ID":7,"Entries":["a\\tb\\n c"]},'
                         '{"ID":8,"Entries":[""]},'
                         '{"ID":9,"Entries":["-"]}]}')
        loaded = load_row_names('ChrModelParam', tmp)
        # A name may not contain the field separator, or the crate's parser would split the line.
        assert loaded == {7: ['a b c']}, loaded
        # An absent file is None ("no checkout"), which main() distinguishes from an empty one.
        assert load_row_names('NpcParam', tmp) is None

    # The truncation the table's own header promises.
    assert len(build([1], {1: ['x' * 200]}, {})[0][1]) == NAME_MAX_CHARS
    print('er-chr-names-gen selftest: ok')
    return 0


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--out', default=None,
                        help=f'write the table here (default {DEFAULT_OUT})')
    parser.add_argument('--moveset', default=MOVESET_TBL)
    parser.add_argument('--rownames', default=ROWNAME_DIR)
    parser.add_argument('--report', action='store_true',
                        help='print coverage and write nothing')
    parser.add_argument('--selftest', action='store_true')
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    chr_ids = moveset_chr_ids(args.moveset)
    chr_model = load_row_names('ChrModelParam', args.rownames)
    npc = load_row_names('NpcParam', args.rownames)
    if chr_model is None or npc is None:
        print(f'no row-name checkout at {args.rownames}; set ER_PARAM_ROWNAME_DIR',
              file=sys.stderr)
        return 2

    rows = build(chr_ids, chr_model, npc)
    if args.report:
        by_source = collections.Counter(source for _, _, source in rows)
        print(f'{len(rows)} creatures; ' + ', '.join(
            f'{count} {source}' for source, count in sorted(by_source.items())))
        for chr_id, name, source in rows:
            if source == 'none':
                print(f'  unnamed: c{chr_id:04}')
        return 0

    out = args.out or DEFAULT_OUT
    # newline='\n' explicitly. Text mode with the default newline=None translates '\n' to
    # os.linesep on write, so the same generator over the same inputs emits CRLF on Windows and LF
    # here -- a 423-line spurious diff in a file whose header says "GENERATED, do not hand-edit".
    # The Rust parser trims line ends, so it would not fail; it would just look like somebody had
    # rewritten the table.
    with open(out, 'w', encoding='utf-8', newline='\n') as handle:
        handle.write(render(rows))
    named = sum(1 for _, name, _ in rows if name != NO_NAME)
    print(f'wrote {out}: {named}/{len(rows)} named')
    return 0


if __name__ == '__main__':
    sys.exit(main())
