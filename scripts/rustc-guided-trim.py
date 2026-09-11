#!/usr/bin/env python3
"""Delete what rustc says is unreachable, using its own diagnostics as the work list.

Why this exists
---------------
A feature trim in this repo is done by gating a call site and then letting the compiler enumerate
what became unreachable, rather than by guessing which helpers a removed feature owned. That
enumeration is already written down -- it is the `is never used` and `unused imports` diagnostics
of a build with `-D warnings`. This turns that output back into edits so the enumeration and the
deletion cannot disagree.

It is deliberately not a general refactoring tool. It only deletes items and imports rustc has
already proven nothing reaches, so a wrong answer here is a compile error on the next pass, never
a silent behaviour change.

Usage
-----
    cargo xwin check --release --target x86_64-pc-windows-msvc -p <crate> > build.log 2>&1
    python3 scripts/rustc-guided-trim.py items build.log
    python3 scripts/rustc-guided-trim.py imports build.log

Run one mode, rebuild, run again: deleting an item usually orphans the import that named it, and
deleting that import usually orphans nothing, so two or three passes converge. Read the diff.
"""

import re
import sys
from collections import defaultdict
from pathlib import Path

DEAD_ITEM = re.compile(
    r'^(?:error|warning)(?:\[E\d+\])?: .*? `([A-Za-z0-9_]+)` is never (?:used|read|constructed).*?\n\s*--> ([^\s:]+):(\d+):',
    re.M | re.S,
)
UNUSED_IMPORT = re.compile(
    r'^(?:error|warning)(?:\[E\d+\])?: unused imports?: (.+?)\n\s*--> ([^\s:]+):(\d+):(\d+)\n(.*?)(?=\n\S|\Z)',
    re.M | re.S,
)
# A line that belongs to the item below it: its doc comment, its ordinary comment, its attributes.
PREAMBLE = re.compile(r'^\s*(///|//!|//|#\[|#!\[)')


def item_end(text, start):
    """Index just past the item beginning at `start` -- its `;`, or the `}` that closes its body.

    Depth counts every bracket kind, not just braces: `static X: [T; N] = [..];` carries a `;`
    inside its own type, and a scanner that only tracked braces ended the item there and left the
    array literal orphaned.
    """
    i = start
    depth = 0
    seen_brace = False
    n = len(text)
    while i < n:
        c = text[i]
        if c == '/' and i + 1 < n and text[i + 1] == '/':
            i = text.find('\n', i)
            if i < 0:
                return n
            continue
        if c == '/' and i + 1 < n and text[i + 1] == '*':
            j = text.find('*/', i + 2)
            i = n if j < 0 else j + 2
            continue
        if c == 'r' and i + 1 < n and text[i + 1] in '#"':
            m = re.match(r'r(#*)"', text[i:])
            if m:
                close = '"' + m.group(1)
                j = text.find(close, i + m.end())
                i = n if j < 0 else j + len(close)
                continue
        if c == '"':
            i += 1
            while i < n and text[i] != '"':
                i += 2 if text[i] == '\\' else 1
            i += 1
            continue
        if c == "'":
            # A char literal, or a lifetime. Only the literal form is skipped whole.
            m = re.match(r"'(\\.|[^'\\])'", text[i:])
            i += m.end() if m else 1
            continue
        if c in '{([':
            if c == '{' and depth == 0:
                seen_brace = True
            depth += 1
        elif c in '})]':
            depth -= 1
            if seen_brace and depth == 0:
                return i + 1
        elif c == ';' and depth == 0:
            return i + 1
        i += 1
    return n


def delete_items(path, lines_1based):
    text = Path(path).read_text(encoding='utf-8')
    offsets = [0]
    for line in text.splitlines(keepends=True):
        offsets.append(offsets[-1] + len(line))
    src = text.splitlines(keepends=True)

    spans = []
    for ln in lines_1based:
        head = ln - 1
        while head > 0 and PREAMBLE.match(src[head - 1]):
            head -= 1
        end = item_end(text, offsets[ln - 1])
        nl = text.find('\n', end - 1)
        spans.append((offsets[head], len(text) if nl < 0 else nl + 1))

    spans.sort()
    merged = []
    for s, e in spans:
        if merged and s <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(merged[-1][1], e))
        else:
            merged.append((s, e))

    out = []
    prev = 0
    for s, e in merged:
        out.append(text[prev:s])
        prev = e
    out.append(text[prev:])
    Path(path).write_text(''.join(out), encoding='utf-8')
    return len(merged)


def drop_imports(path, spans):
    p = Path(path)
    lines = p.read_text(encoding='utf-8').splitlines(keepends=True)
    # Latest first, so an earlier edit cannot move a later one's line and column.
    for line, col, width in sorted(spans, reverse=True):
        idx = line - 1
        text = lines[idx]
        start = col - 1
        end = start + width if width else len(text.rstrip('\n'))
        stripped = text.strip()
        if stripped.startswith(('use ', 'pub use ', 'pub(crate) use ')) and stripped.endswith(';'):
            lines[idx] = ''
            continue
        rest = text[end:]
        trailing = re.match(r',\s*', rest)
        if trailing:
            lines[idx] = text[:start] + text[end + trailing.end():]
        else:
            lead = re.search(r',\s*$', text[:start])
            lines[idx] = (text[: lead.start()] if lead else text[:start]) + text[end:]
        if not lines[idx].strip():
            lines[idx] = ''
    p.write_text(''.join(lines), encoding='utf-8')
    return len(spans)


def main():
    if len(sys.argv) != 3 or sys.argv[1] not in {'items', 'imports'}:
        print(__doc__)
        return 2
    mode, log_path = sys.argv[1], sys.argv[2]
    log = Path(log_path).read_text(encoding='utf-8')

    if mode == 'items':
        work = defaultdict(list)
        for _name, path, line in DEAD_ITEM.findall(log):
            work[path].append(int(line))
        total = sum(delete_items(path, sorted(set(lines))) for path, lines in sorted(work.items()))
        for path, lines in sorted(work.items()):
            print(f'  {path}: {len(set(lines))} item(s)')
        print('deleted', total, 'span(s)')
        return 0

    work = defaultdict(list)
    for _names, path, line, col, body in UNUSED_IMPORT.findall(log):
        caret = re.search(r'^\s*\|\s*(\^+)\s*$', body, re.M)
        work[path].append((int(line), int(col), len(caret.group(1)) if caret else None))
    total = sum(drop_imports(path, spans) for path, spans in sorted(work.items()))
    for path, spans in sorted(work.items()):
        print(f'  {path}: {len(spans)} import(s)')
    print('dropped', total)
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
