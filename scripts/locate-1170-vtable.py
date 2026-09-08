#!/usr/bin/env python3
"""Locate a 1.16.2 C++ vtable in the 1.17 image, then read every slot.

Why a separate tool. A virtual function is often reachable only through its vtable: nothing
`call`s it directly, so `resolve-1170-by-caller-rel32.py` finds no bridge, and its body is a
`ret 0` stub or a two-instruction getter, so `map-rvas-1162-to-1170.py` finds nine equally-good
shape matches. Both tools are working correctly and both are unable to answer. The vtable itself
is the missing witness -- it is an array of pointers, and an array is far more distinctive than
any one of the stubs it points at.

The method. Take the 1.16.2 vtable's slots. Some of their functions are already mapped to 1.17 by
signature or by caller. For each such slot i with a known 1.17 target T', every place in the 1.17
image holding the 8 bytes of T' is a candidate for "slot i lives here", implying a vtable base of
`hit - i*8`. Collect that implication from every mapped slot and take the base that the most slots
agree on. Independent slots voting for one base is the evidence; a base carried by a single slot is
a coincidence waiting to happen and is reported as weak.

Why this is safer than it looks. The vote is over pointer values at fixed strides, so a wrong base
would need several unrelated 1.17 functions to sit at exactly the right multiples of 8 from each
other -- which is the same thing as being the vtable. Once the base is fixed, the remaining slots
are read straight out of the image, and those reads are not predictions at all: they are what the
1.17 binary literally contains, including for the stubs neither other tool could touch.

Superseded for any class with siblings -- Use `find-1170-vtable-by-rtti.py` instead.
Measured 2026-09-01: for `RideManipulator` (1.16.2 `0x142a2c108`) the vote tied at 42 slots
between `0x142a2c8e8` and `0x142a2f118`, and this file returned the first. It was wrong. RTTI
shows `0x142a2f118` is `RideManipulator` and `0x142a2c8e8` is `ChrManipulator` -- the base class,
which of course shares almost every slot with its child. The vote cannot separate a class from its
parent, because slot agreement is exactly what inheritance produces. Use this tool only for a
class whose decorated name RTTI does not carry; otherwise the name chain is exact and free.

The flat images are mapped with file offset == RVA (`.rdata` included), so a vtable VA is read at
`va - 0x140000000` with no section arithmetic. Getting that wrong is a documented past failure --
an earlier note claimed `.rdata` sat at a `+0xE00` offset and had readers landing 3.5 KB off target.
"""
import json, os, socket, struct, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = 0x140000000
OLD_IMAGE = os.path.join(ROOT, "eldenring-deobf.bin")
NEW_IMAGE = os.path.join(ROOT, "eldenring-deobf-1.17.bin")


def mcp(method, params, port):
    req = json.dumps({"id": "1", "method": method, "params": params}).encode()
    with socket.create_connection(("localhost", port), timeout=120) as s:
        s.sendall(struct.pack(">I", len(req)) + req)
        hdr = b""
        while len(hdr) < 4:
            c = s.recv(4 - len(hdr))
            if not c:
                raise IOError("closed")
            hdr += c
        n = struct.unpack(">I", hdr)[0]
        buf = b""
        while len(buf) < n:
            c = s.recv(min(65536, n - len(buf)))
            if not c:
                raise IOError("closed")
            buf += c
    return json.loads(buf.decode("utf-8", "replace")).get("result")


def load_pairs():
    pairs = {}
    for name in (
        "docs/recon/npc-possess-candidates-1170.tsv",
        "docs/recon/npc-possess-resolved-1170.tsv",
        "docs/recon/npc-possess-bridges-1170.tsv",
        "docs/recon/rva-map-1162-to-1170.verified.tsv",
    ):
        path = os.path.join(ROOT, name)
        if not os.path.exists(path):
            continue
        for line in open(path, encoding="utf-8"):
            if line.startswith("#") or not line.strip():
                continue
            f = line.rstrip("\n").split("\t")
            if len(f) < 2 or f[1] in ("-", ""):
                continue
            try:
                o, n = int(f[0], 16), int(f[1], 16)
            except ValueError:
                continue
            pairs.setdefault(o + (BASE if o < BASE else 0), n + (BASE if n < BASE else 0))
    return pairs


def read_slots(image, va, count):
    off = va - BASE
    return [int.from_bytes(image[off + i * 8 : off + i * 8 + 8], "little") for i in range(count)]


def find_qword(image, value):
    needle = value.to_bytes(8, "little")
    out, start = [], 0
    while True:
        i = image.find(needle, start)
        if i < 0:
            break
        out.append(i + BASE)
        start = i + 1
        if len(out) > 64:
            break
    return out


def main():
    if len(sys.argv) < 2:
        sys.exit("usage: locate-1170-vtable.py 0x<vtable_va_1162> [slots]")
    vt = int(sys.argv[1], 0)
    count = int(sys.argv[2], 0) if len(sys.argv) > 2 else 80
    old_image = open(OLD_IMAGE, "rb").read()
    new_image = open(NEW_IMAGE, "rb").read()
    pairs = load_pairs()
    old_slots = read_slots(old_image, vt, count)

    votes = {}
    for i, fn in enumerate(old_slots):
        tgt = pairs.get(fn)
        if not tgt:
            continue
        for hit in find_qword(new_image, tgt):
            votes[hit - i * 8] = votes.get(hit - i * 8, 0) + 1
    if not votes:
        sys.exit(f"no mapped slot in the first {count} of {vt:#x}; map some of its functions first")
    best = max(votes.items(), key=lambda kv: kv[1])
    strength = "STRONG" if best[1] >= 3 else ("WEAK - single witness" if best[1] < 2 else "OK")
    print(f"# 1.16.2 vtable {vt:#x} -> 1.17 {best[0]:#x}  ({best[1]} agreeing slots, {strength})")
    others = sorted((v, k) for k, v in votes.items() if k != best[0])[-3:]
    if others:
        print(f"# runner-up bases: {[(hex(k), v) for v, k in reversed(others)]}")
    new_slots = read_slots(new_image, best[0], count)
    print("# slot\t1.16.2 fn\t1.17 fn\tagrees with known pair")
    for i, (o, n) in enumerate(zip(old_slots, new_slots)):
        known = pairs.get(o)
        mark = "" if known is None else ("YES" if known == n else f"NO (pair said {known:#x})")
        print(f"+{i*8:#06x}\t{o:#x}\t{n:#x}\t{mark}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
