#!/usr/bin/env python3
"""Reliably map a Ghidra-runtime-DUMP address to the DEOBF/live binary address (and back).

WHY THIS EXISTS
---------------
The Ghidra runtime dump (pc_eldenring_runtime.1.16.1) and the deobf/live image
(eldenring-deobf.bin) are NOT byte-identical. Two independent things differ:

  1. LAYOUT: the same function sits at a different VA in each image. The offset
     ("shift" = deobf_va - dump_va) is NOT one constant -- it is piecewise-constant
     PER CODE REGION and drifts across the image (measured: 0 near the base; an
     irregular -0x80..-0x120 staircase through the low .text 0x1401-0x140d; a
     rock-solid -0x20 across 0x140e-0x141e; a rock-solid +0x10 across 0x141f-0x1426;
     messy tail 0x1427+). The historically documented "+0x10"/"-0x10" was just ONE
     region's value -- trusting it elsewhere lands you mid-function and crashes.

  2. RELOCATED OPERANDS: because the code moved, every RIP-relative displacement
     (call/lea/mov [rip+disp32]) and every relative branch target (e8/e9/eb/jcc)
     is re-encoded to a DIFFERENT value. So a raw byte compare of a function
     prologue fails the moment it spans one of these fields.

NOTE: the shift is NOT driven by Arxan. Verified empirically: shift-step boundaries
do not coincide with Arxan stubs (0/457 within 0x40), and regenerating the deobf
image with dearxan produces a byte-identical file -- so dearxan cannot compute the
shift. It is just scattered per-region layout differences between the two images.

HOW THIS TOOL WORKS (driver-agnostic, relocation-aware)
-------------------------------------------------------
We never trust a shift formula. Primary path: decode the instructions at the source
VA with capstone, build a byte pattern in which the relocation-sensitive operand
bytes (RIP-relative disp + relative-branch imm) are WILDCARDED, and search the other
image for the stable opcode/modrm skeleton. A unique match IS the ground-truth
mapping (method "content-unique").

Region assist (on by default; --no-region to disable): a committed per-region shift
table (dump-deobf-shift.regions.tsv) is used to (a) DISAMBIGUATE when several
skeleton matches exist -- the real one sits exactly on the local regional shift
(method "content+region"), and (b) ESTIMATE the shift when there are no source
bytes (zeroed/non-resident dump page) or the code is too short to anchor. Estimates
are returned with verified=False and a "VERIFY with disasm" note -- they can be off
by one region step near a boundary. Content matches are always exact and preferred.

Measured on symbolized (named) functions: ~78% content-verified, ~21% flagged
estimate, ~99%+ resolved overall. Failures collapse to exception funclets / import
thunks with too few stable bytes -- not real lookup targets.

INPUTS (both RVA-aligned: file_offset == VA - 0x140000000)
  - eldenring-deobf.bin  (repo root; authoritative-for-addresses deobf image)
  - dump-exec.bin        (repo root; exported by scripts/ghidra/DumpExecImage.java)

USAGE
  scripts/dump-deobf-shift.py 0x14266def0 [more vas...]   # dump -> deobf (default)
  scripts/dump-deobf-shift.py --reverse 0x14266df00       # deobf -> dump
  scripts/dump-deobf-shift.py --json 0x...                # machine-readable
  scripts/dump-deobf-shift.py --bytes 64 0x...            # decode >=64 bytes of insns

capstone is auto-provisioned via `uv run --with capstone` if not importable.
"""
import argparse, json, os, sys

# --- capstone bootstrap via uv (no persistent install needed) ----------------
try:
    import capstone  # noqa: F401
except ImportError:
    if os.environ.get("_DDS_BOOTSTRAPPED") != "1":
        os.environ["_DDS_BOOTSTRAPPED"] = "1"
        os.execvp("uv", ["uv", "run", "--with", "capstone", "python3",
                         os.path.abspath(__file__)] + sys.argv[1:])
    sys.exit("capstone unavailable and `uv run --with capstone` bootstrap failed")

from capstone import Cs, CS_ARCH_X86, CS_MODE_64
from capstone import x86 as cs_x86

BASE = 0x140000000
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEOBF = os.path.join(ROOT, "eldenring-deobf.bin")
DUMP = os.path.join(ROOT, "dump-exec.bin")

_md = Cs(CS_ARCH_X86, CS_MODE_64)
_md.detail = True


def build_pattern(img, off, want_bytes):
    """Decode instructions at file offset `off`, returning (pat: bytearray,
    mask: bytearray of 1/0) covering >= want_bytes, with relocation-sensitive
    operand bytes wildcarded (mask 0). Returns (pat, mask) or (None, None)."""
    blob = img[off:off + want_bytes + 16]
    pat = bytearray()
    mask = bytearray()
    consumed = 0
    for insn in _md.disasm(bytes(blob), BASE + off):
        ins_bytes = insn.bytes
        m = bytearray([1] * len(ins_bytes))
        enc = insn.encoding
        # RIP-relative displacement bytes -> wildcard
        if enc.disp_offset and enc.disp_size and is_rip_rel(insn):
            for i in range(enc.disp_offset, enc.disp_offset + enc.disp_size):
                if i < len(m):
                    m[i] = 0
        # relative-branch immediate bytes -> wildcard
        if is_rel_branch(insn) and enc.imm_offset and enc.imm_size:
            for i in range(enc.imm_offset, enc.imm_offset + enc.imm_size):
                if i < len(m):
                    m[i] = 0
        pat += bytearray(ins_bytes)
        mask += m
        consumed += len(ins_bytes)
        # Stop at a function-terminating instruction so the signature never crosses
        # into the next (differently-laid-out) function. Include the terminator.
        if is_terminator(insn):
            break
        if consumed >= want_bytes:
            break
    if consumed < 4:
        return None, None
    return pat, mask


def is_terminator(insn):
    m = insn.mnemonic
    if m == "ret" or m == "retf" or m == "int3":
        return True
    # unconditional jmp (relative or indirect) ends a basic-block / often a function
    if m == "jmp":
        return True
    return False


def is_rip_rel(insn):
    for op in insn.operands:
        if op.type == cs_x86.X86_OP_MEM and op.mem.base == cs_x86.X86_REG_RIP:
            return True
    return False


def is_rel_branch(insn):
    g = insn.groups
    if cs_x86.X86_GRP_JUMP in g or cs_x86.X86_GRP_CALL in g or cs_x86.X86_GRP_BRANCH_RELATIVE in g:
        # only relative forms carry an immediate operand
        for op in insn.operands:
            if op.type == cs_x86.X86_OP_IMM:
                return True
    return False


def longest_stable_run(mask):
    best_len = best_start = 0
    cur_start = None
    for i, b in enumerate(mask):
        if b:
            if cur_start is None:
                cur_start = i
            if i - cur_start + 1 > best_len:
                best_len = i - cur_start + 1
                best_start = cur_start
        else:
            cur_start = None
    return best_start, best_len


def masked_find(hay, pat, mask, lo, hi):
    """Find all start positions in hay[lo:hi] where every mask==1 byte of pat
    matches. Uses the longest stable run as a fast anchor."""
    a_start, a_len = longest_stable_run(mask)
    if a_len < 3 or sum(mask) < 6:
        return []  # too little stable structure to anchor/disambiguate reliably
    anchor = bytes(pat[a_start:a_start + a_len])
    hits = []
    i = lo + a_start
    end = hi
    while True:
        j = hay.find(anchor, i, end)
        if j < 0:
            break
        cand = j - a_start
        if cand >= 0 and cand + len(pat) <= len(hay) and verify(hay, pat, mask, cand):
            hits.append(cand)
            if len(hits) > 2:
                break
        i = j + 1
    return hits


def verify(hay, pat, mask, cand):
    for i, mb in enumerate(mask):
        if mb and hay[cand + i] != pat[i]:
            return False
    return True


# --- region shift table (assist): dump_va -> shift, piecewise per region --------
import bisect

REGIONS_PATH = os.path.join(ROOT, "scripts", "dump-deobf-shift.regions.tsv")
_regions = None  # sorted list of (dump_off, shift)


def load_regions():
    global _regions
    if _regions is not None:
        return _regions
    _regions = []
    if os.path.exists(REGIONS_PATH):
        with open(REGIONS_PATH) as f:
            for line in f:
                line = line.strip()
                if not line or line.startswith("#"):
                    continue
                va_s, sh_s = line.split("\t")
                _regions.append((int(va_s, 0) - BASE, int(sh_s, 0)))
        _regions.sort()
    return _regions


def predicted_shift(dump_off):
    """Predicted dump->deobf shift at a dump offset (None if no table / before first entry)."""
    regs = load_regions()
    if not regs:
        return None
    offs = [r[0] for r in regs]
    k = bisect.bisect_right(offs, dump_off) - 1
    return regs[k][1] if k >= 0 else None


def map_va(src_img, dst_img, src_va, want_bytes, window, reverse=False, use_region=True):
    """Map src_va to the other image. `reverse` True means src is deobf (dst is dump).
    Region table assists by disambiguating multiple content matches and (last resort)
    estimating the shift for code too short to match by content."""
    off = src_va - BASE
    if off < 0 or off >= len(src_img):
        return {"src_va": src_va, "ok": False, "error": "src VA outside image"}

    # Region prediction of the dump->deobf shift near this address. For reverse
    # (deobf->dump) the expected src->dst shift is the negative of that.
    pred = predicted_shift(off) if use_region else None
    exp = None if pred is None else (-pred if reverse else pred)

    best_ambig = None
    decode_failed = False
    for wb in (want_bytes, want_bytes * 2, want_bytes * 3):
        pat, mask = build_pattern(src_img, off, wb)
        if pat is None:
            decode_failed = True
            break  # no src bytes (zeroed/non-resident dump page) -> region estimate below
        for win in (window, window * 8, window * 64):
            lo = max(0, off - win)
            hi = min(len(dst_img), off + win + len(pat))
            hits = masked_find(dst_img, pat, mask, lo, hi)
            if len(hits) == 1:
                return _ok(src_va, hits[0], pat, mask, win, "content-unique")
            if len(hits) > 1:
                # Region-disambiguate: keep the hit whose shift equals the predicted
                # regional shift (the shift is locally constant, so the true match
                # sits exactly on it; spurious skeleton matches do not).
                if exp is not None:
                    onreg = [h for h in hits if (h - off) == exp]
                    if len(onreg) == 1:
                        return _ok(src_va, onreg[0], pat, mask, win, "content+region")
                best_ambig = hits
                break  # grow signature
    # No unique content match. Last resort: region estimate (clearly flagged).
    if use_region and exp is not None:
        dst_va = src_va + exp
        why = "no src bytes (dump page zeroed/non-resident)" if decode_failed else "no content match"
        return {"src_va": src_va, "dst_va": dst_va, "shift": exp, "ok": True,
                "method": "region-estimate", "verified": False,
                "note": "%s; shift from region table -- VERIFY with disasm" % why}
    if decode_failed:
        err = "could not decode instructions at src (no region table for estimate)"
    elif best_ambig:
        err = "ambiguous content match, no region table to disambiguate"
    else:
        err = "no relocation-masked match (grew signature + window)"
    return {"src_va": src_va, "ok": False, "error": err}


def _ok(src_va, dst_off, pat, mask, win, method):
    dst_va = dst_off + BASE
    return {"src_va": src_va, "dst_va": dst_va, "shift": dst_va - src_va, "ok": True,
            "method": method, "verified": True, "decoded_bytes": len(pat),
            "stable_run": longest_stable_run(mask)[1], "window": win}


def main():
    ap = argparse.ArgumentParser(description="Map dump<->deobf addresses by relocation-aware byte content.")
    ap.add_argument("vas", nargs="+", help="VAs (hex 0x... or decimal)")
    ap.add_argument("--reverse", action="store_true", help="inputs are DEOBF VAs, map to DUMP")
    ap.add_argument("--bytes", dest="want", type=lambda s: int(s, 0), default=40,
                    help="min instruction bytes to decode for the signature (default 40)")
    ap.add_argument("--window", type=lambda s: int(s, 0), default=0x800,
                    help="initial +- search window (default 0x800)")
    ap.add_argument("--no-region", dest="region", action="store_false",
                    help="disable the region-table assist (content match only; no estimates)")
    ap.add_argument("--json", action="store_true", help="machine-readable output")
    args = ap.parse_args()

    for p in (DEOBF, DUMP):
        if not os.path.exists(p):
            sys.exit("missing image: %s (deobf via scripts/dearxan-deobfuscate.rs; "
                     "dump-exec.bin via scripts/ghidra/DumpExecImage.java)" % p)
    deobf = open(DEOBF, "rb").read()
    dump = open(DUMP, "rb").read()
    if args.reverse:
        src, dst, sn, dn = deobf, dump, "deobf", "dump"
    else:
        src, dst, sn, dn = dump, deobf, "dump", "deobf"

    out = []
    for v in args.vas:
        r = map_va(src, dst, int(v, 0), args.want, args.window,
                   reverse=args.reverse, use_region=args.region)
        r["direction"] = "%s->%s" % (sn, dn)
        out.append(r)

    if args.json:
        print(json.dumps(out, indent=2))
        return
    for r in out:
        if not r["ok"]:
            print("%s 0x%x -> FAILED: %s" % (sn, r["src_va"], r["error"]))
        elif r.get("verified", True):
            print("%s 0x%x -> %s 0x%x   shift=%+#x   [%s]" % (
                sn, r["src_va"], dn, r["dst_va"], r["shift"], r.get("method", "content")))
        else:
            print("%s 0x%x -> %s 0x%x   shift=%+#x   [%s] %s" % (
                sn, r["src_va"], dn, r["dst_va"], r["shift"],
                r.get("method", "estimate"), r.get("note", "")))


if __name__ == "__main__":
    main()
