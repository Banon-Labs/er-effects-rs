#!/usr/bin/env python3
"""Resolve a 1.16.2 -> 1.17 function pair by decoding the rel32 out of an already-mapped caller.

Why this exists. `map-rvas-1162-to-1170.py` matches a relocation-masked byte signature. That
works on a function with a distinctive body and fails on the ones this project needs most: leaf
getters, `ret 0` stubs and thunks are shape-ambiguous -- nine functions in 1.17 look exactly like
`mov rax,[rcx+0x18]; ret`, and the mapper correctly refuses to pick one. It reports
"9 shape matches, none at the nearest anchor's delta", which is an honest shrug, not an address.

The call graph answers what the bytes cannot. If a caller C is already mapped to C', and C calls
the target F, then F' is simply whatever C' calls in the same position. That is not a similarity
score; it is the identity the two images themselves record. It is how `SphereCastClosest` and
`ChrCtrl::GetPhysicsPosition` were recovered.

The alignment rule, and why it is index-based rather than offset-based. A tempting shortcut is
`F' = C' + (F_callsite - C)` -- the same byte offset into the mapped caller. That is wrong the
moment 1.17 inserts or removes a single instruction anywhere earlier in C, which is exactly the
drift this whole exercise is about (`STEP_MoveMap` gained two instructions at index 873). So the
alignment is by call INDEX: decode every `call`/`jmp` rel32 in both bodies in order, and require
the two lists to have the same length before trusting position i. A length mismatch means the
body changed shape and the correspondence is unproven -- the script says so and declines, rather
than returning a plausible address.

CORROBORATION, not a single witness. A target called from several mapped callers gets one vote
per caller, and they must agree. Agreement across independent callers is much stronger evidence
than any byte pattern, because each caller is a separate derivation. A split vote is reported as
a conflict and resolves nothing.
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


def func_at(va, port):
    r = mcp("getFunctionByAddress", {"address": f"{va:x}"}, port)
    if not r or "entry" not in r:
        return None
    return int(r["entry"], 16), r.get("size", 0)


def calls_in(image, entry, size):
    """Every rel32 call/jmp target in [entry, entry+size), in address order.

    Decoded rather than byte-scanned: an `e8` byte inside a displacement or immediate is not a
    call, and treating it as one would silently shift every later index by-- which is precisely
    the correspondence this file depends on.
    """
    from capstone import CS_ARCH_X86, CS_MODE_64, Cs

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    off = entry - BASE
    out = []
    for insn in md.disasm(image[off : off + size], entry):
        if insn.mnemonic in ("call", "jmp") and len(insn.bytes) == 5 and insn.bytes[0] in (0xE8, 0xE9):
            out.append(int(insn.op_str, 16) if insn.op_str.startswith("0x") else None)
    return out


def load_pairs():
    pairs = {}
    for name in (
        "docs/recon/npc-possess-candidates-1170.tsv",
        "docs/recon/npc-possess-bridges-1170.tsv",
        "docs/recon/npc-possess-resolved-1170.tsv",
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


def resolve(target, pairs, old_image, new_image):
    xr = mcp("getXrefsTo", {"address": f"{target:x}", "limit": 200}, 8765) or {}
    callers = []
    for it in xr.get("items", []):
        if not it.get("isCall"):
            continue
        site = int(it["fromAddress"], 16)
        fa = func_at(site, 8765)
        if fa and fa[0] not in [c[0] for c in callers]:
            callers.append(fa)
    votes, notes = {}, []
    for entry, size in callers:
        new_entry = pairs.get(entry)
        if not new_entry:
            continue
        nf = func_at(new_entry, 8767)
        if not nf:
            continue
        old_calls = calls_in(old_image, entry, size)
        new_calls = calls_in(new_image, nf[0], nf[1])
        if len(old_calls) != len(new_calls):
            notes.append(f"{entry:#x}: call-count {len(old_calls)}!={len(new_calls)}, declined")
            continue
        if target not in old_calls:
            continue
        for i, t in enumerate(old_calls):
            if t == target:
                cand = new_calls[i]
                votes[cand] = votes.get(cand, 0) + 1
                notes.append(f"{entry:#x}->{nf[0]:#x} call#{i} => {cand:#x}")
    return votes, notes, len(callers)


def main():
    targets = [int(v, 0) for v in sys.argv[1:]]
    if not targets:
        sys.exit("usage: resolve-1170-by-caller-rel32.py 0x<va> [...]")
    old_image = open(OLD_IMAGE, "rb").read()
    new_image = open(NEW_IMAGE, "rb").read()
    pairs = load_pairs()
    print(f"# known pairs available as bridges: {len(pairs)}")
    for t in targets:
        votes, notes, ncallers = resolve(t, pairs, old_image, new_image)
        if not votes:
            print(f"{t:#x}\t-\tUNRESOLVED\t{ncallers} callers, no mapped bridge")
        elif len(votes) == 1:
            cand, n = next(iter(votes.items()))
            print(f"{t:#x}\t{cand:#x}\tcaller rel32, {n} agreeing caller(s)\t{'; '.join(notes[:3])}")
        else:
            print(f"{t:#x}\t-\tCONFLICT {[(hex(k),v) for k,v in votes.items()]}\t{'; '.join(notes[:4])}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
