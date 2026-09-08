#!/usr/bin/env python3
"""Screen struct-field offsets for 1.16.2 -> 1.17 drift by 32-bit displacement frequency.

For an offset >= 0x80, x86-64 encodes `[reg+off]` with a 4-byte little-endian displacement.
Counting that exact 4-byte sequence in each de-Arxan'd image is crude (it also matches data and
non-displacement bytes) but the comparison is informative: a count that is identical in both
images is corroboration that nothing referencing that offset moved, and a count that collapses
toward zero in 1.17 while non-zero in 1.16.2 is a red flag worth a targeted accessor check.

Not proof either way -- it is a triage screen to decide where to spend a positive comparison.
"""
import os, struct, argparse, json

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IMG = {'1162': os.path.join(ROOT, 'eldenring-deobf.bin'),
       '1170': os.path.join(ROOT, 'eldenring-deobf-1.17.bin')}

def count(data, needle):
    n, i = 0, data.find(needle)
    while i != -1:
        n += 1
        i = data.find(needle, i + 1)
    return n

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('offsets', nargs='+', help='hex offsets, e.g. 0x1e8 0xb08')
    a = ap.parse_args()
    d1 = open(IMG['1162'], 'rb').read()
    d2 = open(IMG['1170'], 'rb').read()
    print(f"{'offset':>10} {'1.16.2':>9} {'1.17':>9}  verdict")
    for o in a.offsets:
        v = int(o, 16)
        if v < 0x80:
            print(f"{o:>10} {'-':>9} {'-':>9}  skipped (1-byte disp form; too noisy)")
            continue
        needle = struct.pack('<I', v)
        c1, c2 = count(d1, needle), count(d2, needle)
        if c1 == c2:
            note = 'same count'
        else:
            pct = abs(c1 - c2) / max(c1, 1) * 100
            note = f'DIFFERS ({pct:.0f}%)' if pct > 5 else 'near-same'
        print(f"{o:>10} {c1:>9} {c2:>9}  {note}")

if __name__ == '__main__':
    main()
