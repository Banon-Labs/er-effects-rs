#!/usr/bin/env python3
"""Brace-matched Rust function splitter, string/comment aware.

Shared helper for the installer/raw-write audits. Rust source is not regex-parsable
in the small ways that matter here: a `?` inside a string literal, a `return` inside
a doc comment, and a `{` inside a char literal all defeat a line-oriented scan. This
masks literals/comments to spaces (preserving offsets) and then brace-matches, so a
caller can ask a real question about a real function body.

Not a parser. It finds `fn NAME ... { ... }` spans and nothing else; nested fns are
reported as their own entries, which is what the audits want.
"""
import re

def mask_source(src: str) -> str:
    """Parallel string with comments/string/char literal content blanked, offsets preserved."""
    out = []
    i = 0
    n = len(src)
    while i < n:
        c = src[i]
        if c == '/' and i + 1 < n and src[i + 1] == '/':
            j = src.find('\n', i)
            if j == -1:
                j = n
            out.append(' ' * (j - i))
            i = j
        elif c == '/' and i + 1 < n and src[i + 1] == '*':
            depth = 1
            j = i + 2
            while j < n and depth:
                if src[j] == '/' and j + 1 < n and src[j + 1] == '*':
                    depth += 1
                    j += 2
                elif src[j] == '*' and j + 1 < n and src[j + 1] == '/':
                    depth -= 1
                    j += 2
                else:
                    j += 1
            out.append(' ' * (j - i))
            i = j
        elif c == 'r' and re.match(r'r#*"', src[i:i + 8]):
            m = re.match(r'r(#*)"', src[i:])
            hashes = m.group(1)
            end = src.find('"' + hashes, i + len(m.group(0)))
            end = n if end == -1 else end + 1 + len(hashes)
            out.append(' ' * (end - i))
            i = end
        elif c == '"':
            j = i + 1
            while j < n:
                if src[j] == '\\':
                    j += 2
                    continue
                if src[j] == '"':
                    j += 1
                    break
                j += 1
            out.append(' ' * (j - i))
            i = j
        elif c == "'":
            m = re.match(r"'(\\.|[^\\'])'", src[i:])
            if m:
                out.append(' ' * len(m.group(0)))
                i += len(m.group(0))
            else:
                out.append(c)
                i += 1
        else:
            out.append(c)
            i += 1
    return ''.join(out)


def functions(path):
    """Yield (name, line, body_src, body_mask) for every `fn` with a body in `path`."""
    src = open(path, encoding='utf-8', errors='replace').read()
    mask = mask_source(src)
    for m in re.finditer(r'\bfn\s+([A-Za-z0-9_]+)', mask):
        i = m.end()
        depth_par = 0
        while i < len(mask):
            ch = mask[i]
            if ch == '(':
                depth_par += 1
            elif ch == ')':
                depth_par -= 1
            elif ch == '{' and depth_par == 0:
                break
            elif ch == ';' and depth_par == 0:
                i = -1
                break
            i += 1
        if i == -1 or i >= len(mask):
            continue
        depth = 0
        j = i
        while j < len(mask):
            if mask[j] == '{':
                depth += 1
            elif mask[j] == '}':
                depth -= 1
                if depth == 0:
                    break
            j += 1
        line = src[:m.start()].count('\n') + 1
        yield (m.group(1), line, src[m.start():j + 1], mask[m.start():j + 1])


def selftest():
    import tempfile, os
    sample = '''
fn a() -> Result<(), String> {
    // return Err("in a comment");
    let s = "fn fake() { ? }";
    let c = '{';
    do_thing()?;
    Ok(())
}
fn b() { let _ = 1; }
'''
    fd, p = tempfile.mkstemp(suffix='.rs')
    os.write(fd, sample.encode())
    os.close(fd)
    fns = list(functions(p))
    os.unlink(p)
    names = [f[0] for f in fns]
    assert names == ['a', 'b'], names
    # the ? inside the string literal must be masked; the real one must survive
    assert fns[0][3].count('?') == 1, fns[0][3].count('?')
    assert 'return Err' not in fns[0][3], 'comment leaked into mask'
    assert fns[1][2].strip().endswith('}')
    print('rustfn selftest OK: 2 fns, 1 real `?`, comment+string masked')


if __name__ == '__main__':
    selftest()
