#!/usr/bin/env python3
"""Static call-graph climber over the deobf flat image (eldenring-deobf.bin).

Given a target VA and a depth, recursively resolve CALLERS (up) or CALLEES
(down) of a function, grounding each `call rel32` site to its containing
function entry -- so we stop chasing xrefs one level at a time by hand.

Mapped image: file offset == RVA, base 0x140000000 (see disas-deobf.sh).
Only direct `E8` rel32 calls are followed (the reliable, unambiguous edges);
indirect/vtable dispatch is NOT in the graph -- use find-xrefs.py's
absolute-pointer mode for vtable slots. `E9` tail-jmps are optionally included
(--jmp) since FromSoft thunks tail-call.

Usage:
  call-graph.py <va_hex> [--up N | --down N] [--jmp] [--max-nodes M]
  --up N    climb callers to depth N (default: --up 3)
  --down N  climb callees to depth N
Examples:
  call-graph.py 0x1401efc00 --up 4
  call-graph.py 0x140615340 --down 2
"""
import sys

BASE = 0x140000000
IMG = __file__.rsplit("/", 1)[0] + "/../eldenring-deobf.bin"

# Common MSVC x64 function-prologue opening byte patterns, used to validate a
# grounded entry. Order-independent; we just check the entry starts with one.
PROLOGUES = (
    b"\x40\x53", b"\x40\x55", b"\x40\x56", b"\x40\x57",  # rex push rbx/rbp/rsi/rdi
    b"\x53", b"\x55", b"\x56", b"\x57",                  # push rbx/rbp/rsi/rdi
    b"\x48\x89\x5c\x24", b"\x48\x89\x4c\x24",            # mov [rsp+x], rbx/rcx
    b"\x48\x89\x54\x24", b"\x48\x89\x44\x24",            # mov [rsp+x], rdx/rax
    b"\x48\x83\xec", b"\x48\x81\xec",                    # sub rsp, imm
    b"\x4c\x8b\xdc", b"\x48\x8b\xc4",                    # mov r11,rsp / mov rax,rsp
    b"\x41\x54", b"\x41\x55", b"\x41\x56", b"\x41\x57",  # push r12..r15
    b"\x44\x88", b"\x44\x89", b"\x48\x8b\xc1", b"\xb8",
)


def load() -> bytes:
    with open(IMG, "rb") as f:
        return f.read()


def build_call_edges(data: bytes, follow_jmp: bool):
    """One pass: map target_va -> [call_site_va, ...] for every E8 (and, if
    follow_jmp, E9) rel32. Returns (callers_of, sites) where sites is a sorted
    list of (site_va, target_va) for callee extraction."""
    callers_of: dict[int, list[int]] = {}
    sites: list[tuple[int, int]] = []
    n = len(data)
    i = 0
    while i < n - 5:
        op = data[i]
        if op == 0xE8 or (follow_jmp and op == 0xE9):
            disp = int.from_bytes(data[i + 1 : i + 5], "little", signed=True)
            tgt = BASE + i + 5 + disp
            if BASE <= tgt < BASE + n:
                site = BASE + i
                callers_of.setdefault(tgt, []).append(site)
                sites.append((site, tgt))
            i += 5
            continue
        i += 1
    sites.sort()
    return callers_of, sites


def ground_entry(data: bytes, site_va: int) -> tuple[int, bool]:
    """Scan back from a call-site VA to the containing function entry. Entries
    are 0xcc-padded by MSVC; find the nearest preceding 0xcc run and take the
    first byte after it, validating against a known prologue. Returns
    (entry_va, validated)."""
    off = site_va - BASE
    lo = max(0, off - 0x6000)
    # Walk back to the closest 0xcc that precedes a plausible entry.
    p = off
    while p > lo:
        if data[p - 1] == 0xCC:
            # consume the cc run
            e = p
            while e < off and data[e] == 0xCC:
                e += 1
            if e <= off and any(
                data[e : e + len(sig)] == sig for sig in PROLOGUES
            ):
                return BASE + e, True
        p -= 1
    # Fallback: nearest 0xcc boundary even if prologue didn't validate.
    p = off
    while p > lo:
        if data[p - 1] == 0xCC:
            e = p
            while e < off and data[e] == 0xCC:
                e += 1
            return BASE + e, False
        p -= 1
    return BASE + lo, False


def callees_of(data: bytes, sites, entry_va: int) -> list[int]:
    """Direct callees of the function starting at entry_va: scan its body (to
    the next 0xcc padding after a ret) for E8 targets, via the prebuilt sites
    list (binary search the span)."""
    import bisect

    off = entry_va - BASE
    n = len(data)
    end = off
    # find function end: first 0xc3 (ret) followed by 0xcc within a bound
    span = min(n, off + 0x6000)
    i = off
    while i < span:
        if data[i] == 0xC3 and i + 1 < n and data[i + 1] == 0xCC:
            end = i + 1
            break
        i += 1
    else:
        end = span
    lo = bisect.bisect_left(sites, (entry_va, 0))
    hi = bisect.bisect_left(sites, (BASE + end, 0))
    out = []
    seen = set()
    for _site, tgt in sites[lo:hi]:
        if tgt not in seen:
            seen.add(tgt)
            out.append(tgt)
    return out


def main() -> int:
    args = sys.argv[1:]
    if not args:
        print(__doc__, file=sys.stderr)
        return 2
    va = int(args[0], 16)
    direction = "up"
    depth = 3
    follow_jmp = False
    max_nodes = 400
    i = 1
    while i < len(args):
        a = args[i]
        if a == "--up":
            direction = "up"; depth = int(args[i + 1]); i += 2
        elif a == "--down":
            direction = "down"; depth = int(args[i + 1]); i += 2
        elif a == "--jmp":
            follow_jmp = True; i += 1
        elif a == "--max-nodes":
            max_nodes = int(args[i + 1]); i += 2
        else:
            print(f"unknown arg: {a}", file=sys.stderr); return 2

    data = load()
    callers_of, sites = build_call_edges(data, follow_jmp)

    printed = [0]
    visited: set[int] = set()

    def walk(node_va: int, lvl: int, prefix: str):
        if printed[0] >= max_nodes:
            return
        if direction == "up":
            sites_in = callers_of.get(node_va, [])
            grounded = []
            seen = set()
            for s in sites_in:
                e, ok = ground_entry(data, s)
                if e not in seen:
                    seen.add(e)
                    grounded.append((e, ok, s))
            kids = grounded
        else:
            kids = [(t, True, None) for t in callees_of(data, sites, node_va)]
        if lvl >= depth:
            if kids:
                print(f"{prefix}  ... ({len(kids)} more at depth limit)")
            return
        for e, ok, site in kids:
            if printed[0] >= max_nodes:
                print(f"{prefix}  ... (max-nodes {max_nodes} reached)")
                return
            printed[0] += 1
            mark = "" if ok else " ?entry"
            via = f" (call@0x{site:x})" if site is not None else ""
            cyc = " [cycle]" if e in visited else ""
            print(f"{prefix}+- 0x{e:x}{mark}{via}{cyc}")
            if e in visited:
                continue
            visited.add(e)
            walk(e, lvl + 1, prefix + "   ")

    arrow = "callers (up)" if direction == "up" else "callees (down)"
    print(f"call-graph 0x{va:x} {arrow} depth={depth} jmp={follow_jmp}")
    visited.add(va)
    walk(va, 0, "")
    print(f"[{printed[0]} node(s) printed]")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
