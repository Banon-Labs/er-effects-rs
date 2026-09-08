#!/usr/bin/env python3
"""Find byte patterns in the deobfuscated ER mapped image and report their VAs.

The deobf image is a flat mapped image: file offset == RVA, image base 0x140000000.

Usage:
    python3 scripts/find-deobf-bytes.py <hexpattern> [<hexpattern> ...]

A pattern is a hex string; use "??" for a wildcard byte, e.g.:
    python3 scripts/find-deobf-bytes.py "0fb683a8820000" "488b0d????????"

Override the image with ER_DEOBF_BIN.

The scan stops at 64 hits and says so ("hits=64+ (TRUNCATED ...)"), because a pattern loose
enough to match dozens of sites is usually the wrong pattern rather than a long answer. Raise the
cap with ER_DEOBF_MAX_HITS when you genuinely want the whole list -- but note that "hits=N" with
no `+` is the only form that means "these are all of them", and an enumeration read off a
truncated list is a negative nobody has actually proved. For "who touches struct field +N",
reach for `scripts/find-deobf-field-access.py` instead: it decodes ModRM rather than matching one
encoding, so it covers every base register and prefix in one pass and does not truncate.
"""
import os
import sys

DEFAULT_IMG = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "eldenring-deobf.bin"
)
IMG = os.environ.get("ER_DEOBF_BIN", DEFAULT_IMG)
BASE = 0x140000000
MAX_HITS = int(os.environ.get("ER_DEOBF_MAX_HITS", "64"))


def parse(pat: str):
    pat = pat.replace(" ", "").lower()
    if len(pat) % 2:
        raise SystemExit("pattern %r has odd length" % pat)
    pairs = [pat[i : i + 2] for i in range(0, len(pat), 2)]
    want = bytes(0 if p == "??" else int(p, 16) for p in pairs)
    mask = bytes(0 if p == "??" else 0xFF for p in pairs)
    return want, mask


def search(data: bytes, want: bytes, mask: bytes):
    n = len(want)
    anchor = next((i for i, m in enumerate(mask) if m), None)
    if anchor is None:
        raise SystemExit("pattern is entirely wildcards")
    needle = want[anchor : anchor + 1]
    hits = []
    start = 0
    while True:
        idx = data.find(needle, start)
        if idx < 0:
            break
        off = idx - anchor
        start = idx + 1
        if off < 0 or off + n > len(data):
            continue
        if all(
            mask[j] == 0 or data[off + j] == want[j] for j in range(n)
        ):
            hits.append(off)
            if len(hits) >= MAX_HITS:
                break
    return hits


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    if not os.path.exists(IMG):
        print("missing image: %s (set ER_DEOBF_BIN)" % IMG, file=sys.stderr)
        return 1
    data = open(IMG, "rb").read()
    for pat in sys.argv[1:]:
        want, mask = parse(pat)
        hits = search(data, want, mask)
        # The `+` is load-bearing. A bare count reads as "this is the complete set", and an
        # agent that concluded "nothing else writes this field" off a silently-capped list would
        # be proving a negative out of a truncated sample.
        truncated = len(hits) >= MAX_HITS
        print(
            "%s  hits=%d%s  %s%s"
            % (
                pat,
                len(hits),
                "+" if truncated else "",
                " ".join(hex(h + BASE) for h in hits),
                "  (TRUNCATED at ER_DEOBF_MAX_HITS=%d -- NOT the whole set)" % MAX_HITS
                if truncated
                else "",
            )
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
