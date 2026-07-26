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
import argparse, importlib.util, json, os, re, sys
from collections import Counter

# --- capstone bootstrap via uv (no persistent install needed) ----------------
# capstone is provisioned at runtime by `uv run --with capstone`; it is NOT in the base
# interpreter Pyright resolves against, so probe with find_spec (a bare import would be an
# unresolved-import error) and the two `from capstone` imports below carry a documented ignore.
if importlib.util.find_spec("capstone") is None:
    if os.environ.get("_DDS_BOOTSTRAPPED") != "1":
        os.environ["_DDS_BOOTSTRAPPED"] = "1"
        os.execvp("uv", ["uv", "run", "--with", "capstone", "python3",
                         os.path.abspath(__file__)] + sys.argv[1:])
    sys.exit("capstone unavailable and `uv run --with capstone` bootstrap failed")

from capstone import Cs, CS_ARCH_X86, CS_MODE_64  # pyright: ignore[reportMissingImports]
from capstone import x86 as cs_x86  # pyright: ignore[reportMissingImports]

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


# --- post-match reliability validation ---------------------------------------
# These guards exist because a region-table-assisted pick (method "content+region"
# or "region-estimate") can be silently WRONG: the region table can be stale, and a
# genuine byte-content chunk of one image can appear at a MID-INSTRUCTION offset in
# the other (functions differ by a single-byte encoding upstream). A consumer that
# treats such a VA as a MinHook/patch site writes a jmp over live mid-instruction
# bytes and crashes the game. So region-assisted results are never a clean confident
# answer: they are flagged UNRELIABLE and the process exits non-zero. A "content-unique"
# match (a unique relocation-masked skeleton match in a bounded window) is ground
# truth and is left untouched.
#
# NOTE: a plain linear-decode "is this an instruction boundary?" test is NOT usable as
# a gate on content-unique matches -- the deobf image has overlapping-instruction /
# thunk artifacts before ~60% of real function starts (e.g. an `e8` byte one before the
# prologue makes a linear `call` straddle the true start), so such a gate false-alarms
# on the majority of correct matches. The straddle signal is therefore used ONLY to
# ENRICH the warning on results that are already flagged region-tier, never to reclassify
# a content-unique match.

_HEX = re.compile(r"0x[0-9a-fA-F]+")


def _norm_insn(insn):
    """(mnemonic, operand-shape) with relocation-sensitive fields wildcarded, so two
    instructions that differ only by a RIP-relative displacement or a relative-branch
    target compare EQUAL (they are the "same" instruction across the two images)."""
    ops = insn.op_str
    ops = re.sub(r"rip [+-] 0x[0-9a-fA-F]+", "rip+X", ops)
    if is_rel_branch(insn):
        ops = _HEX.sub("TGT", ops)
    return (insn.mnemonic, ops)


def _decode_norm(img, off, n):
    out = []
    for insn in _md.disasm(bytes(img[off:off + n * 16 + 16]), BASE + off):
        out.append(_norm_insn(insn))
        if len(out) >= n:
            break
    return out


def leading_agreement(src_img, src_off, dst_img, dst_off, n=4):
    """How many leading instructions (relocation-normalized) match between the source
    VA and the candidate. Returns 'k/total'. For a genuine content match this is total;
    for a bad region-ESTIMATE (pointing at unrelated code) it is typically 0."""
    s = _decode_norm(src_img, src_off, n)
    d = _decode_norm(dst_img, dst_off, n)
    tot = min(len(s), len(d))
    if tot == 0:
        return "0/0"
    k = 0
    for a, b in zip(s, d):
        if a == b:
            k += 1
        else:
            break
    return "%d/%d" % (k, tot)


def _frac(s):
    try:
        a, b = s.split("/")
        return int(a), int(b)
    except Exception:
        return 0, 0


def linear_straddle(img, off, lookback=24):
    """Return the VA of an instruction that STRADDLES `off` (starts strictly before,
    ends strictly after) according to a strong, consistent backward linear-decode
    consensus, else None. A hit means `off` is NOT on a clean instruction boundary in
    this image -- do not blindly patch there. (Fires on true mid-instruction landings
    AND on real function starts preceded by overlapping-thunk artifacts; both are worth
    warning about on an already-unreliable region-tier result.)"""
    boundary = 0
    straddlers = Counter()
    for k in range(1, lookback + 1):
        start = off - k
        if start < 0:
            continue
        for insn in _md.disasm(bytes(img[start:off + 16]), BASE + start):
            ia = insn.address - BASE
            ie = ia + insn.size
            if ia == off:
                boundary += 1
                break
            if ia < off < ie:
                straddlers[ia] += 1
                break
            if ia > off:
                break
    if not straddlers:
        return None
    strad = sum(straddlers.values())
    dom_ia, dom_cnt = straddlers.most_common(1)[0]
    if strad >= 6 and strad >= 3 * boundary and dom_cnt >= max(4, strad * 0.5):
        return dom_ia
    return None


def finalize(r, src_img, dst_img, src_off):
    """Attach reliability metadata to a successful map result. content-unique stays a
    clean, verified, reliable answer. Every region-assisted method is downgraded to
    UNRELIABLE (reliable=False) with loud flags so a patch-site consumer cannot be
    silently misled; main() turns any unreliable/failed result into a non-zero exit."""
    if not r.get("ok"):
        return r
    method = r.get("method", "content-unique")
    if method == "content-unique":
        r["reliable"] = True
        return r

    # Region-assisted: NOT a clean confident patch target.
    r["reliable"] = False
    r["verified"] = False
    flags = []
    dst_off = r["dst_va"] - BASE
    if method == "content+region":
        flags.append("UNVERIFIED-region")
        r.setdefault("note", "region-table-assisted pick among multiple content "
                             "matches (region table can be stale) -- VERIFY with disasm")
    elif method == "region-estimate":
        flags.append("UNRELIABLE-estimate")
    else:
        flags.append("UNVERIFIED")

    la = leading_agreement(src_img, src_off, dst_img, dst_off)
    r["leading_match"] = la
    k, tot = _frac(la)
    if tot and k == 0:
        flags.append("UNRELIABLE-nomatch")

    strad = linear_straddle(dst_img, dst_off)
    if strad is not None:
        flags.append("UNRELIABLE-midinsn")
        r["boundary_straddle"] = "0x%x" % (strad + BASE)
        r["note"] = ("candidate is NOT on a clean instruction boundary (linear decode "
                     "straddles it at 0x%x) -- do NOT patch here without disasm" % (strad + BASE))
    r["flags"] = flags
    return r


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
                return finalize(_ok(src_va, hits[0], pat, mask, win, "content-unique"),
                                src_img, dst_img, off)
            if len(hits) > 1:
                # Region-disambiguate: keep the hit whose shift equals the predicted
                # regional shift (the shift is locally constant, so the true match
                # sits exactly on it; spurious skeleton matches do not). This pick is
                # only as good as the region table -- finalize() marks it UNRELIABLE.
                if exp is not None:
                    onreg = [h for h in hits if (h - off) == exp]
                    if len(onreg) == 1:
                        return finalize(_ok(src_va, onreg[0], pat, mask, win, "content+region"),
                                        src_img, dst_img, off)
                best_ambig = hits
                break  # grow signature
    # No unique content match. Last resort: region estimate (clearly flagged).
    if use_region and exp is not None:
        dst_va = src_va + exp
        why = "no src bytes (dump page zeroed/non-resident)" if decode_failed else "no content match"
        return finalize({"src_va": src_va, "dst_va": dst_va, "shift": exp, "ok": True,
                         "method": "region-estimate", "verified": False,
                         "note": "%s; shift from region table -- VERIFY with disasm" % why},
                        src_img, dst_img, off)
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

    # A result is a clean, patch-safe answer ONLY if it resolved AND is reliable
    # (content-unique). Anything else (region-assisted or FAILED) forces a non-zero
    # exit so a programmatic patch-site consumer cannot treat it as confident.
    unreliable = [r for r in out if (not r.get("ok")) or (not r.get("reliable"))]

    if args.json:
        print(json.dumps(out, indent=2))
        if unreliable:
            sys.exit(3)
        return

    for r in out:
        if not r["ok"]:
            print("%s 0x%x -> FAILED: %s" % (sn, r["src_va"], r["error"]))
        elif r.get("reliable"):
            # genuine content-unique match -- unchanged output format
            print("%s 0x%x -> %s 0x%x   shift=%+#x   [%s]" % (
                sn, r["src_va"], dn, r["dst_va"], r["shift"], r.get("method", "content")))
        else:
            flags = " ".join(r.get("flags", ["UNRELIABLE"]))
            print("%s 0x%x -> %s 0x%x   shift=%+#x   [%s | %s] %s" % (
                sn, r["src_va"], dn, r["dst_va"], r["shift"],
                r.get("method", "estimate"), flags, r.get("note", "")))

    # Loud stderr warning so the hazard is impossible to miss even when stdout is parsed.
    for r in unreliable:
        if not r.get("ok"):
            print("WARNING: %s 0x%x did NOT resolve (%s) -- do NOT patch." % (
                sn, r["src_va"], r.get("error", "")), file=sys.stderr)
        else:
            print("WARNING: %s 0x%x -> %s 0x%x is UNRELIABLE [%s]: %s "
                  "VERIFY with scripts/disas-deobf.sh before patching/hooking." % (
                      sn, r["src_va"], dn, r["dst_va"],
                      " ".join(r.get("flags", ["UNRELIABLE"])), r.get("note", "")),
                  file=sys.stderr)
    if unreliable:
        sys.exit(3)


if __name__ == "__main__":
    main()
