#!/usr/bin/env python3
"""Build a whole-image call graph from a Ghidra function list plus the flat de-Arxan'd image.

Ghidra's analysis found ~366k functions per image -- a node set `.pdata` cannot supply (it
declares nothing for 5.55 MB of `.text`). This turns that node set into a directed graph by
decoding each function's own bytes out of the flat image and resolving every direct branch that
leaves the function onto another node.

Deliberate choices, because a graph is only comparable across two images if the same rule built
both sides:

* Decode window is `[entry, min(entry + ghidra_size, next_entry))`. Ghidra's `size` is the body's
  address-set cardinality, so for a chunked function it exceeds the contiguous span; capping at
  the next entry stops the decode running into a neighbour and inventing its callees.
* `call rel32` and `call rel16` are edges. A `jmp` is an edge only when its target lands outside
  the decode window -- an intra-function `jmp` is a basic block, not a callee, and counting them
  once inflated a leaf census by 2.2x (see agent-w4-leaves.md).
* Conditional jumps are never edges.
* Indirect calls are counted, never resolved.
* A branch to an address that is not a Ghidra function entry is counted as `out_of_graph`, not
  silently dropped, so the caller can see how much of the body the graph does not model.
* Each node also gets a body HASH: a 64-bit digest of the whole body's instruction sequence with
  every numeric literal blanked. Displacements and immediates are exactly what a version bump
  moves, so they must not be in the digest; what is left is the body's shape, over its whole
  declared length rather than a fixed-length prefix. That length anchor is the point -- a
  fixed-length masked prefix is how an impostor at 0xaec480 came back identical over 56
  instructions while the correct pair matched over 9.

Output is a pickle:
  entries   sorted tuple of node VAs
  size      {va: ghidra size}
  name      {va: ghidra name}
  callees   {va: tuple((target_va, 'c'|'j'), ... in body order)}
  stats     {va: (n_insn, n_direct, n_indirect, n_out_of_graph, decoded_bytes)}
  bodyhash  {va: 64-bit digest of the numeric-blanked instruction sequence, or None}

  python3 scripts/build-callgraph-from-ghidra-funcs.py \
      --image eldenring-deobf-1.17.bin --funcs funcs-1170.tsv --out cg-1170.pickle
"""
import argparse
import os
import hashlib
import pickle
import re
import sys
import time

try:
    import capstone
except ImportError:  # bootstrap under uv, per AGENTS.md (no system pip)
    os.execvp("uv", ["uv", "run", "--with", "capstone", "python3", *sys.argv])

BASE = 0x140000000
NUM = re.compile(r"0x[0-9a-f]+")


def load_funcs(path):
    entries = []
    size = {}
    name = {}
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            parts = line.rstrip("\n").split("\t")
            if len(parts) < 3:
                continue
            va = int(parts[0], 16)
            entries.append(va)
            size[va] = int(parts[1])
            name[va] = parts[2]
    entries.sort()
    return entries, size, name


def build(image_path, funcs_path, out_path, verbose=True):
    image = open(image_path, "rb").read()
    entries, size, name = load_funcs(funcs_path)
    node = set(entries)
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    md.detail = False
    disasm_lite = md.disasm_lite

    callees = {}
    stats = {}
    bodyhash = {}
    n = len(entries)
    t0 = time.time()
    for i, va in enumerate(entries):
        rva = va - BASE
        if rva < 0 or rva >= len(image):
            callees[va] = ()
            stats[va] = (0, 0, 0, 0, 0)
            bodyhash[va] = None
            continue
        limit = va + size.get(va, 0)
        if i + 1 < n:
            limit = min(limit, entries[i + 1])
        if limit <= va:
            limit = va + 1
        end_rva = min(limit - BASE, len(image))
        code = image[rva:end_rva]
        out = []
        n_insn = n_dir = n_ind = n_oog = 0
        last_end = rva
        digest = hashlib.blake2b(digest_size=8)
        for addr, isize, mnem, ops in disasm_lite(code, va):
            n_insn += 1
            digest.update(mnem.encode())
            digest.update(b" ")
            digest.update(NUM.sub("#", ops).encode())
            digest.update(b"\n")
            last_end = addr - BASE + isize
            if mnem == "call":
                if ops.startswith("0x"):
                    n_dir += 1
                    tgt = int(ops, 16)
                    if tgt in node:
                        out.append((tgt, "c"))
                    else:
                        n_oog += 1
                else:
                    n_ind += 1
            elif mnem == "jmp":
                if ops.startswith("0x"):
                    tgt = int(ops, 16)
                    if va <= tgt < limit:
                        continue  # intra-function block, not a callee
                    n_dir += 1
                    if tgt in node:
                        out.append((tgt, "j"))
                    else:
                        n_oog += 1
                else:
                    n_ind += 1
        callees[va] = tuple(out)
        stats[va] = (n_insn, n_dir, n_ind, n_oog, last_end - rva)
        # A body that mostly failed to decode is not a fingerprint of anything.
        bodyhash[va] = digest.digest() if n_insn and last_end - rva >= (end_rva - rva) - 15 else None
        if verbose and (i % 50000 == 0):
            print(f"{i}/{n}  {time.time()-t0:.0f}s", flush=True)

    with open(out_path, "wb") as fh:
        pickle.dump({"entries": tuple(entries), "size": size, "name": name,
                     "callees": callees, "stats": stats, "bodyhash": bodyhash,
                     "image": os.path.basename(image_path)}, fh, protocol=4)
    if verbose:
        edges = sum(len(v) for v in callees.values())
        print(f"nodes={n} edges={edges} in {time.time()-t0:.0f}s -> {out_path}", flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--image", required=True)
    ap.add_argument("--funcs", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    build(a.image, a.funcs, a.out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
